# `tcp-proxy`

## Usage

```
tcp-proxy - Tokio based, flexible TCP Proxy implementation.

Usage: tcp-proxy [OPTIONS] <CONFIG_PATH>

Arguments:
  <CONFIG_PATH>

Options:
      --debug    Display debug logs
  -h, --help     Print help
  -V, --version  Print version
```

## Configuration File Example

1. Single target port.

    ```json
    [
        {
            "ip": "192.168.0.1",
            "port": 80,
            "target_port": 8080
        }
    ]
    ```

2. Multiple target port.

    ```json
    [
        {
            "ip": "192.168.50.3",
            "port": {
                "start": 1,
                "end": 10000
            },
            "target_port": {
                "start": 10001,
                "end": 20000
            }
        }
    ]
    ```

> [!IMPORTANT]
>
> Provide `port` and `target_port` with the same length, otherwise this tool will not be able to proxy all the services!
