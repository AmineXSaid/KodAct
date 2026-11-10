"""Print one line per service, in start order (dependencies first)."""
import json


def start_order(services):
    by_name = {s["name"]: s for s in services}
    done, order = set(), []

    def visit(name):
        if name in done:
            return
        for dep in by_name[name]["depends_on"]:
            visit(dep)
        done.add(name)
        order.append(by_name[name])

    for s in services:
        visit(s["name"])
    return order


if __name__ == "__main__":
    with open("services.json") as f:
        config = json.load(f)
    for s in start_order(config["services"]):
        print(f'{s["name"]}:{s["port"]} ({s["owner"]})')
