import os

from . import config, settings


def resolve_port(cfg):
    """The port to bind: APP_PORT, else the server section, else the default."""
    if "APP_PORT" in os.environ:
        return int(os.environ["APP_PORT"])
    server = cfg.get("server")
    if server and "port" in server:
        return int(server["port"])
    return int(cfg.get("port", settings.DEFAULT_PORT))


def main():
    port = resolve_port(config.load())
    print(f"listening on {port}")
    raise SystemExit("this exercise does not start a real server")


if __name__ == "__main__":
    main()
