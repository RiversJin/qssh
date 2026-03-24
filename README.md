# qssh

QUIC-based SSH proxy with connection migration support.

qssh tunnels SSH traffic over the QUIC protocol, providing resilience on unstable networks without modifying your SSH client or server. QUIC's connection migration allows SSH sessions to survive network switches (e.g., Wi-Fi to cellular) seamlessly.

## Features

- **Connection migration** — sessions survive network changes (IP/port rebinding)
- **Packet loss resilience** — QUIC's built-in retransmission handles lossy networks
- **Configurable timeouts** — idle timeout, keepalive interval, buffer size all adjustable via CLI or config file
- **Certificate management** — auto-generated self-signed certs persisted to disk, with TOFU (Trust-On-First-Use) verification by default
- **SNI-based routing** — one server can proxy to multiple SSH backends based on hostname
- **NixOS module** — deploy as a systemd service via flake

## Installation

### From source

```sh
cargo install --path .
```

### Nix

```sh
nix run github:YOUR_USERNAME/qssh
```

Or add to your flake inputs:

```nix
inputs.qssh.url = "github:YOUR_USERNAME/qssh";
```

## Quick start

### Server

Run on the remote host, proxying to local sshd:

```sh
qssh server -l 0.0.0.0:4433 -p 127.0.0.1:22
```

### Client

Add to `~/.ssh/config`:

```
Host myserver
    HostName your.server.ip
    ProxyCommand qssh client quic://%h:4433
```

Then connect as usual:

```sh
ssh myserver
```

## Configuration

qssh supports a TOML config file. Pass it with `-c`:

```sh
qssh -c qssh.toml server
```

Example config:

```toml
[client]
buffer_size = 16384
max_idle_timeout_ms = 60000
keepalive_interval_secs = 5
cert_verify = "tofu"
known_hosts = "~/.config/qssh/known_hosts"

[server]
listen = "0.0.0.0:4433"
proxy_to = "127.0.0.1:22"
buffer_size = 16384
max_idle_timeout_ms = 60000
keepalive_interval_secs = 5
cert_dir = "~/.config/qssh"

# SNI-based routing
[server.routes]
"host-a.example.com" = "10.0.0.1:22"
"host-b.example.com" = "10.0.0.2:22"
```

Configuration priority: CLI flags > config file > defaults.

### Certificate verification

The client supports three modes via `--cert-verify`:

| Mode | Description |
|------|-------------|
| `tofu` (default) | Trust on first use. Records server fingerprint on first connection, rejects changes afterward. Similar to SSH `known_hosts`. |
| `none` | Skip all verification. Use for testing only. |
| `/path/to/ca.pem` | Verify against a CA certificate. |

### Server certificate persistence

The server auto-generates a self-signed TLS certificate on first run and saves it to `~/.config/qssh/` (or the path specified by `--cert-dir`). Subsequent restarts reuse the same certificate. You can also provide your own cert with `--cert` and `--key`.

## NixOS deployment

```nix
{
  inputs.qssh.url = "github:YOUR_USERNAME/qssh";

  outputs = { nixpkgs, qssh, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        qssh.nixosModules.default
        {
          services.qssh = {
            enable = true;
            openFirewall = true;
            settings.server = {
              listen = "0.0.0.0:4433";
              proxy_to = "127.0.0.1:22";
            };
          };
        }
      ];
    };
  };
}
```

## CLI reference

```
qssh server [OPTIONS]
  -l, --listen <ADDR>              Listen address [default: 0.0.0.0:4433]
  -p, --proxy-to <ADDR>            Upstream SSH address [default: 127.0.0.1:22]
      --cert <PATH>                TLS certificate file (PEM)
      --key <PATH>                 TLS private key file (PEM)
      --cert-dir <PATH>            Directory for auto-generated certs
      --cert-sans <SANS>           SANs for auto-generated cert (comma-separated)
      --max-idle-timeout-ms <MS>   Max idle timeout [default: 60000]
      --keepalive-interval-secs <S> Keep-alive interval [default: 5]
      --buffer-size <BYTES>        I/O buffer size [default: 16384]

qssh client [OPTIONS] <URL>
  <URL>                            Server URL (quic://host:port)
  -b, --bind <ADDR>                Local bind address
      --cert-verify <MODE>         Verification: "tofu", "none", or CA cert path [default: tofu]
      --known-hosts <PATH>         TOFU fingerprint store
      --max-idle-timeout-ms <MS>   Max idle timeout [default: 60000]
      --keepalive-interval-secs <S> Keep-alive interval [default: 5]
      --buffer-size <BYTES>        I/O buffer size [default: 16384]

Global options:
  -c, --config <PATH>              Config file (TOML)
      --log-level <LEVEL>          Log level [default: warn]
```

## Testing

```sh
cargo test
```

The integration tests include a UDP chaos proxy that simulates packet loss, latency, and connection migration without requiring root privileges.

## License

MIT
