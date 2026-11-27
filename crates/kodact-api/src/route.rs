//! Several endpoints in one session: each request goes to the endpoint that serves its model,
//! so switching models (`/model`, `--model`, a sub-agent's or the summary's model) works
//! whatever endpoint the model is on, and however many are set up.
//!
//! A model goes to, in this order:
//! 1. the endpoint its id names: `work/kgpt-reasoning-text` goes to the endpoint called `work`,
//!    as `kgpt-reasoning-text` (an id whose first part names no endpoint, as OpenRouter's
//!    `openai/gpt-4o`, is left whole);
//! 2. the first endpoint whose `models` patterns match it (`*` matches any run of characters);
//! 3. the first endpoint whose model list has it. The lists are asked for once, the first time a
//!    model is matched by nothing above, and kept for the session;
//! 4. the default endpoint (the one the session's own settings choose, else the first).
//!
//! Each endpoint keeps its own concurrency limit (the session wraps each one), so a gateway
//! that throttles slows only the models it serves.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use kodact_types::MessagesRequest;
use tokio_util::sync::CancellationToken;

use crate::{auth, ApiError, EventStream, Provider};

/// How long routing waits for an endpoint's model list before it sends the request to the
/// default endpoint instead.
pub const LIST_WAIT: Duration = Duration::from_secs(5);

/// One endpoint of a [`RoutedProvider`].
pub struct Route {
    /// The endpoint's name in the settings (`endpoints.<name>`), used as an id's prefix.
    pub name: String,
    pub provider: Arc<dyn Provider>,
    /// Model ids, or patterns with `*`, this endpoint serves.
    pub models: Vec<String>,
    /// The ids its model list gave, once asked (`Some(vec![])` when it lists none or failed).
    listed: Mutex<Option<Vec<String>>>,
}

impl Route {
    pub fn new(name: impl Into<String>, provider: Arc<dyn Provider>, models: Vec<String>) -> Self {
        Route { name: name.into(), provider, models, listed: Mutex::new(None) }
    }

    fn matches(&self, model: &str) -> bool {
        self.models.iter().any(|p| wildcard(p, model))
    }

    fn lists(&self, model: &str) -> bool {
        self.listed.lock().unwrap().as_ref().is_some_and(|l| l.iter().any(|m| m == model))
    }

    fn listed_yet(&self) -> bool {
        self.listed.lock().unwrap().is_some()
    }

    async fn fetch(&self) -> Option<Result<Vec<String>, ApiError>> {
        let got = tokio::time::timeout(LIST_WAIT, self.provider.list_models()).await.ok().flatten();
        let ids = match &got {
            Some(Ok(ids)) => ids.clone(),
            _ => vec![],
        };
        *self.listed.lock().unwrap() = Some(ids);
        got
    }
}

/// `pattern` matches `text`, where `*` stands for any run of characters (none included).
pub fn wildcard(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !text.starts_with(first) || text.len() < first.len() + last.len() || !text.ends_with(last) {
        return false;
    }
    let mut rest = &text[first.len()..text.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    true
}

/// A provider made of several endpoints: each request goes to the one serving its model.
pub struct RoutedProvider {
    routes: Vec<Route>,
    default: usize,
}

impl RoutedProvider {
    /// `routes` in the order they are tried; `default` takes what no rule places.
    pub fn new(routes: Vec<Route>, default: usize) -> Self {
        assert!(!routes.is_empty(), "a routed provider needs an endpoint");
        let default = default.min(routes.len() - 1);
        RoutedProvider { routes, default }
    }

    pub fn routes(&self) -> &[Route] {
        &self.routes
    }

    /// The endpoint and the id to send it, from what is known now (no request made).
    fn pick(&self, model: &str) -> Option<(usize, String)> {
        if let Some((prefix, rest)) = model.split_once('/') {
            if let Some(i) = self.routes.iter().position(|r| r.name == prefix) {
                if !rest.is_empty() {
                    return Some((i, rest.to_string()));
                }
            }
        }
        if let Some(i) = self.routes.iter().position(|r| r.matches(model)) {
            return Some((i, model.to_string()));
        }
        self.routes.iter().position(|r| r.lists(model)).map(|i| (i, model.to_string()))
    }

    fn pick_or_default(&self, model: &str) -> (usize, String) {
        self.pick(model).unwrap_or_else(|| (self.default, model.to_string()))
    }

    /// As [`Self::pick`], asking the endpoints that weren't asked yet for their lists when
    /// nothing else places the model.
    async fn route(&self, model: &str) -> (usize, String) {
        if let Some(found) = self.pick(model) {
            return found;
        }
        let unasked: Vec<&Route> = self.routes.iter().filter(|r| !r.listed_yet()).collect();
        if !unasked.is_empty() {
            futures::future::join_all(unasked.iter().map(|r| r.fetch())).await;
        }
        self.pick_or_default(model)
    }

    /// The endpoint serving `model`, as far as is known now.
    pub fn route_of(&self, model: &str) -> &Route {
        &self.routes[self.pick_or_default(model).0]
    }
}

#[async_trait::async_trait]
impl Provider for RoutedProvider {
    fn name(&self) -> &str {
        self.routes[self.default].provider.name()
    }

    async fn stream(&self, mut request: MessagesRequest, cancel: CancellationToken) -> Result<EventStream, ApiError> {
        let (i, model) = self.route(&request.model).await;
        request.model = model;
        self.routes[i].provider.stream(request, cancel).await
    }

    async fn count_tokens(&self, request: &MessagesRequest) -> Option<u64> {
        let (i, model) = self.pick_or_default(&request.model);
        let mut request = request.clone();
        request.model = model;
        self.routes[i].provider.count_tokens(&request).await
    }

    fn resends_reasoning(&self, model: &str) -> bool {
        let (i, model) = self.pick_or_default(model);
        self.routes[i].provider.resends_reasoning(&model)
    }

    async fn probe(&self) -> Option<Result<usize, ApiError>> {
        self.routes[self.default].provider.probe().await
    }

    fn origin(&self) -> Option<auth::Origin> {
        self.routes[self.default].provider.origin()
    }

    fn base_url(&self) -> Option<String> {
        self.routes[self.default].provider.base_url()
    }

    fn serving(&self, model: &str) -> Option<Arc<dyn Provider>> {
        Some(self.route_of(model).provider.clone())
    }

    fn endpoint_for(&self, model: &str) -> Option<(String, String)> {
        let (i, id) = self.pick_or_default(model);
        Some((self.routes[i].name.clone(), id))
    }

    /// Every endpoint's models, in endpoint order. An id an earlier endpoint already lists is
    /// given with its endpoint's name (`work/model`), which routes it there. `None` when no
    /// endpoint lists its models; an error only when every one that answered failed.
    async fn list_models(&self) -> Option<Result<Vec<String>, ApiError>> {
        let answers = futures::future::join_all(self.routes.iter().map(|r| r.fetch())).await;
        let mut ids: Vec<String> = vec![];
        let mut first_error = None;
        let mut any_ok = false;
        for (route, answer) in self.routes.iter().zip(answers) {
            match answer {
                Some(Ok(list)) => {
                    any_ok = true;
                    for id in list {
                        if ids.contains(&id) {
                            ids.push(format!("{}/{id}", route.name));
                        } else {
                            ids.push(id);
                        }
                    }
                }
                Some(Err(e)) => {
                    first_error.get_or_insert(e);
                }
                None => {}
            }
        }
        match (any_ok, first_error) {
            (true, _) => Some(Ok(ids)),
            (false, Some(e)) => Some(Err(e)),
            (false, None) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{MockProvider, MockTurn};

    fn mock(models: Option<&[&str]>) -> Arc<MockProvider> {
        let m = Arc::new(MockProvider::new([MockTurn::text("a"), MockTurn::text("b"), MockTurn::text("c")]));
        if let Some(ids) = models {
            m.set_models(ids);
        }
        m
    }

    fn request(model: &str) -> MessagesRequest {
        serde_json::from_value(serde_json::json!({"model": model, "max_tokens": 10, "messages": []})).unwrap()
    }

    async fn send(p: &RoutedProvider, model: &str) {
        let _ = p.stream(request(model), CancellationToken::new()).await.unwrap();
    }

    fn sent(m: &MockProvider) -> Vec<String> {
        m.requests().into_iter().map(|r| r.model).collect()
    }

    #[test]
    fn patterns_match_with_stars() {
        assert!(wildcard("kgpt-*", "kgpt-reasoning-text"));
        assert!(wildcard("*-flash", "gemini-2.5-flash"));
        assert!(wildcard("gpt-*-mini", "gpt-4o-mini"));
        assert!(wildcard("*", "anything"));
        assert!(!wildcard("kgpt-*", "deep-thinking"));
        assert!(!wildcard("gpt-*-mini", "gpt-4o"));
        assert!(wildcard("exact", "exact") && !wildcard("exact", "exactly"));
    }

    /// Switching between models on different endpoints sends each request to the endpoint that
    /// serves it, by name, by pattern and by the endpoints' own lists, with any number of them.
    #[tokio::test]
    async fn each_model_goes_to_the_endpoint_that_serves_it() {
        let (work, google, local, lab) = (
            mock(Some(&["deep-thinking"])),
            mock(None),
            mock(Some(&["qwen3-coder"])),
            mock(Some(&["deep-thinking", "lab-only"])),
        );
        let p = RoutedProvider::new(
            vec![
                Route::new("work", work.clone(), vec!["kgpt-*".into()]),
                Route::new("google", google.clone(), vec!["gemini-*".into()]),
                Route::new("local", local.clone(), vec![]),
                Route::new("lab", lab.clone(), vec![]),
            ],
            0,
        );
        send(&p, "kgpt-reasoning-text").await; // pattern
        send(&p, "gemini-2.5-pro").await; // pattern
        send(&p, "qwen3-coder").await; // local's list
        send(&p, "lab/deep-thinking").await; // named: lab, though work lists it first
        send(&p, "lab-only").await; // lab's list
        send(&p, "deep-thinking").await; // the first endpoint that lists it
        send(&p, "unknown-model").await; // nowhere: the default
        assert_eq!(sent(&work), ["kgpt-reasoning-text", "deep-thinking", "unknown-model"]);
        assert_eq!(sent(&google), ["gemini-2.5-pro"]);
        assert_eq!(sent(&local), ["qwen3-coder"]);
        assert_eq!(sent(&lab), ["deep-thinking", "lab-only"]);
    }

    /// An id whose first part names no endpoint (OpenRouter's `openai/gpt-4o`) is sent whole.
    #[tokio::test]
    async fn a_slash_that_names_no_endpoint_is_part_of_the_id() {
        let router = mock(Some(&["openai/gpt-4o"]));
        let p = RoutedProvider::new(
            vec![Route::new("home", mock(None), vec![]), Route::new("or", router.clone(), vec![])],
            0,
        );
        send(&p, "openai/gpt-4o").await;
        assert_eq!(sent(&router), ["openai/gpt-4o"]);
    }

    /// `/model` lists every endpoint's models; an id already listed is given with its endpoint.
    #[tokio::test]
    async fn the_list_has_every_endpoints_models() {
        let p = RoutedProvider::new(
            vec![
                Route::new("work", mock(Some(&["a", "b"])), vec![]),
                Route::new("silent", mock(None), vec![]),
                Route::new("lab", mock(Some(&["b", "c"])), vec![]),
            ],
            0,
        );
        assert_eq!(p.list_models().await.unwrap().unwrap(), ["a", "b", "lab/b", "c"]);
        // And a listed id routes where the list says.
        assert!(Arc::ptr_eq(&p.serving("lab/b").unwrap(), &p.routes()[2].provider));
        assert!(Arc::ptr_eq(&p.serving("c").unwrap(), &p.routes()[2].provider));
    }

    /// An endpoint whose model list never comes, beside working ones.
    struct Hangs;

    #[async_trait::async_trait]
    impl Provider for Hangs {
        fn name(&self) -> &str {
            "openai"
        }
        async fn stream(&self, _: MessagesRequest, _: CancellationToken) -> Result<EventStream, ApiError> {
            Err(ApiError::Cancelled)
        }
        async fn list_models(&self) -> Option<Result<Vec<String>, ApiError>> {
            std::future::pending().await
        }
    }

    /// Fifty endpoints, one of which never answers: every model reaches its own endpoint, the
    /// lists are asked for in parallel and once, and the hanging one costs one [`LIST_WAIT`] at
    /// most, not one per endpoint or per request.
    #[tokio::test(start_paused = true)]
    async fn fifty_endpoints_and_one_that_hangs() {
        let mocks: Vec<Arc<MockProvider>> = (0..50).map(|i| mock(Some(&[format!("model-{i}").as_str()]))).collect();
        let mut routes: Vec<Route> =
            mocks.iter().enumerate().map(|(i, m)| Route::new(format!("ep{i}"), m.clone(), vec![])).collect();
        routes.insert(7, Route::new("dead", Arc::new(Hangs), vec![]));
        let p = RoutedProvider::new(routes, 0);
        let start = tokio::time::Instant::now();
        for i in [49, 0, 25, 3, 49] {
            send(&p, &format!("model-{i}")).await;
        }
        assert!(start.elapsed() <= LIST_WAIT + Duration::from_millis(50), "{:?}", start.elapsed());
        assert_eq!(sent(&mocks[49]), ["model-49", "model-49"]);
        assert_eq!(sent(&mocks[0]), ["model-0"]);
        assert_eq!(sent(&mocks[25]), ["model-25"]);
        assert_eq!(sent(&mocks[3]), ["model-3"]);
        // By name too, and every endpoint's models in the list.
        send(&p, "ep12/anything").await;
        assert_eq!(sent(&mocks[12]), ["anything"]);
        let all = p.list_models().await.unwrap().unwrap();
        assert_eq!(all.len(), 50, "{all:?}");
    }

    /// The lists are asked for once: later requests for unplaced models don't ask again.
    #[tokio::test]
    async fn the_lists_are_asked_for_once() {
        let a = mock(Some(&["x"]));
        let p = RoutedProvider::new(vec![Route::new("a", a.clone(), vec![])], 0);
        send(&p, "y").await;
        assert!(p.routes()[0].listed_yet());
        a.set_models(&["y"]);
        send(&p, "z").await;
        assert!(!p.routes()[0].lists("y"), "not asked again");
    }
}
