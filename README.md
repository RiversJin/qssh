# qssh

QUIC-based SSH proxy with connection migration support.

qssh tunnels SSH traffic over the QUIC protocol, providing resilience on unstable networks without modifying your SSH client or server. QUIC's connection migration allows SSH sessions to survive network switches (e.g., Wi-Fi to cellular) seamlessly.

Inspired by [quicssh-rs](https://github.com/oowl/quicssh-rs), rewritten from scratch with improved architecture and configurability.

## Improvements over quicssh-rs

| | quicssh-rs | qssh |
|---|---|---|
| Timeouts | Hardcoded (60s idle, 1s keepalive) | Fully configurable via CLI and config file |
| Certificate management | Regenerated on every server restart | Persisted to disk, reused across restarts |
| Certificate verification | None (all certs accepted blindly) | TOFU by default, optional CA verification |
| I/O buffers | Fixed 2KB | Configurable, default 16KB |
| Concurrency model | `tokio::select!` branches (can block each other) | Separate `tokio::spawn` tasks per direction |
| Logging | log4rs | tracing (structured, to stderr) |
| QUIC/TLS stack | quinn 0.10 / rustls 0.21 | quinn 0.11 / rustls 0.23 |
| Configuration | CLI only | Three-layer: defaults -> TOML config -> CLI flags |
| Shutdown | Basic signal handling | Graceful connection draining |
| Deployment | Manual | Nix flake with NixOS module |

## How it works

qssh consists of two components:

- **Server**: listens for incoming QUIC connections and forwards them to an upstream SSH server over TCP. Runs on the remote host alongside sshd.
- **Client**: acts as an SSH `ProxyCommand`. Connects to the qssh server over QUIC and bridges stdin/stdout to the QUIC stream. SSH is unaware that the transport layer has changed.

```
┌──────────┐      QUIC/UDP       ┌──────────┐      TCP       ┌──────────┐
│ SSH      │ ──── stdin/stdout ──│ qssh     │ ──────────────> │ sshd     │
│ client   │                     │ client   │                 │          │
└──────────┘                     └──────────┘                 └──────────┘
              ProxyCommand          :4433                       :22
```

Because QUIC runs over UDP with built-in encryption (TLS 1.3), connection migration, and congestion control, your SSH sessions become resilient to:

- Network switches (Wi-Fi -> cellular, VPN reconnects)
- Brief network outages (up to the configured idle timeout)
- High packet loss environments

The SSH protocol itself is unchanged -- all existing SSH features (port forwarding, agent forwarding, key auth) work transparently.

## Installation

### From source

```sh
cargo install --path .
```

### Nix (run directly)

```sh
nix run github:RiversJin/qssh
```

### Nix (add to system)

Add to your flake inputs:

```nix
inputs.qssh.url = "github:RiversJin/qssh";
```

Then either use the overlay:

```nix
nixpkgs.overlays = [ inputs.qssh.overlays.default ];
# pkgs.qssh is now available
```

Or reference the package directly:

```nix
environment.systemPackages = [ inputs.qssh.packages.${system}.qssh ];
```

## Quick start

### 1. Start the server

On the remote host (where sshd is running):

```sh
qssh server -l 0.0.0.0:4433 -p 127.0.0.1:22
```

Make sure UDP port 4433 is open in your firewall.

### 2. Configure the client

Add to your local `~/.ssh/config`:

```
Host myserver
    HostName your.server.ip
    ProxyCommand qssh client quic://%h:4433
```

### 3. Connect

```sh
ssh myserver
```

On first connection, qssh will record the server's certificate fingerprint (TOFU). Subsequent connections verify the fingerprint matches.

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

# SNI-based routing to different SSH backends
[server.routes]
"host-a.example.com" = "10.0.0.1:22"
"host-b.example.com" = "10.0.0.2:22"
```

Configuration priority: CLI flags > config file > defaults.

### Certificate verification

The client supports three verification modes via `--cert-verify`:

| Mode | Description |
|------|-------------|
| `tofu` (default) | Trust on first use. Records the server's certificate fingerprint on first connection and rejects any changes afterward. Works like SSH's `known_hosts`. |
| `none` | Skip all verification. Suitable for testing only. |
| `/path/to/ca.pem` | Standard CA verification against a provided certificate. |

### Server certificate persistence

The server auto-generates a self-signed TLS certificate on first run and saves it to `~/.config/qssh/` (or the path specified by `--cert-dir`). Subsequent restarts reuse the same certificate, so client TOFU fingerprints remain valid. You can also provide your own certificate with `--cert` and `--key`.

### SNI-based routing

A single qssh server can proxy to multiple SSH backends based on the hostname (SNI) the client connects with. Configure routes in the `[server.routes]` section of the config file. Connections that don't match any route use the default `proxy_to` address.

## NixOS deployment

qssh provides a NixOS module for declarative deployment as a systemd service:

```nix
# flake.nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    qssh.url = "github:RiversJin/qssh";
  };

  outputs = { nixpkgs, qssh, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        qssh.nixosModules.default
        {
          services.qssh = {
            enable = true;
            openFirewall = true;  # opens UDP 4433
            logLevel = "info";
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

The module provides:

- `services.qssh.enable` -- enable the systemd service
- `services.qssh.openFirewall` -- automatically open the UDP port
- `services.qssh.port` -- UDP port for the firewall rule (default: 4433)
- `services.qssh.logLevel` -- log verbosity (default: "info")
- `services.qssh.settings` -- TOML configuration (merged into the config file)

The service runs with systemd hardening (DynamicUser, ProtectSystem, PrivateTmp) and stores certificates in `/var/lib/qssh/`.

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

The integration tests include a UDP chaos proxy that simulates packet loss, latency, and connection migration without requiring root privileges or `tc netem`.

## License

MIT
