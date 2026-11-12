from shop.server import listen_address, worker_count

if __name__ == "__main__":
    host, port = listen_address()
    print(f"shop: {worker_count()} workers on {host}:{port}")
