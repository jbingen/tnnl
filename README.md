# 🚇 tnnl

> Expose localhost to the internet. Single binary, no account required.

![demo](https://vhs.charm.sh/vhs-7x43LFVT4jrZOnRC8oNBJn.gif)

---

## Quickstart

The public server at [tnnl.run](https://tnnl.run) is free to use:

```sh
curl -fsSL https://tnnl.run/install.sh | sh
tnnl http 3000
# → https://abc12345.tnnl.run is live
```

Every request is logged as it comes in:

```
00:05  #1  GET    /                          200  16ms
00:09  #2  GET    /api/users                 200  20ms
00:14  #3  POST   /api/webhooks              201  14ms
```

## Install

**One-line** (Linux / macOS):

```sh
curl -fsSL https://tnnl.run/install.sh | sh
```

**Prebuilt binary**: [releases page](https://github.com/jbingen/tnnl/releases)

**From source**:

```sh
cargo install --path .
```

## Debugging webhooks

The main reason to reach for tnnl over a quick SSH tunnel. Pass `--inspect` to see full headers and body for every request and response:

```sh
tnnl http 3000 --inspect
```

```
00:14  #3  POST   /api/webhooks              201  14ms

  →  POST /api/webhooks  #3
     Content-Type: application/json
     Stripe-Signature: t=1234567890,v1=abc123...

     {
       "type": "charge.succeeded",
       "data": { "amount": 9900, "currency": "usd" }
     }

  ←  201 Created
     Content-Type: application/json

     {"received": true}
```

Every request is saved locally. Fix your handler and replay without touching the sender:

```sh
tnnl replay 3
```

IDs persist across restarts so you can replay old requests after a fresh start.

## Config file

Persistent settings live in `~/.tnnl.toml`:

```toml
server    = "tnnl.run"
token     = "your-secret"  # omit for open servers
subdomain = "myapp"        # same URL every time
auth      = "user:pass"    # HTTP basic auth on the tunnel
```

CLI flags always win over the config file.

## Protecting your tunnel

Gate the exposed URL with HTTP basic auth. Unauthenticated requests are rejected before they touch your local service:

```sh
tnnl http 3000 --auth user:pass
```

## How it works

One binary, two modes. `tnnl server` runs on a VPS, `tnnl http` runs on your machine.

1. Client connects over TCP and authenticates with HMAC-SHA256 - the secret never crosses the wire
2. Server assigns a subdomain (random, or pin one with `--subdomain`)
3. Incoming HTTP traffic is routed by `Host` header to the right client
4. Everything flows over a single multiplexed connection via [yamux](https://github.com/libp2p/rust-yamux) - no TCP handshake per request
5. Client proxies to localhost and pipes the response back

502 if localhost isn't up. Exponential backoff reconnect if the connection drops.

## Self-hosting

```sh
tnnl server --domain tunnel.example.com
# or with a shared secret
tnnl server --domain tunnel.example.com --token supersecret
```

Omitting `--token` makes it an open server. Either way, abuse protection is always on: per-IP rate limiting, per-IP tunnel caps, global tunnel limit.

### Server flags


| Flag             | Default  | Description                                     |
| ---------------- | -------- | ----------------------------------------------- |
| `--domain`       | required | Base domain for tunnel subdomains               |
| `--token`        | none     | Secret clients must know - omit for open server |
| `--control-port` | `9443`   | Port for client connections                     |
| `--http-port`    | `8080`   | Port for public HTTP traffic                    |


### TLS

tnnl terminates plain HTTP on `--http-port`. Put Caddy or nginx in front for TLS with a wildcard cert.

**Caddy:**

```
*.tunnel.example.com {
    tls {
        dns cloudflare {env.CF_API_TOKEN}
    }
    reverse_proxy localhost:8080
}
```

**nginx:**

```nginx
server {
    listen 443 ssl;
    server_name *.tunnel.example.com;

    ssl_certificate     /etc/letsencrypt/live/tunnel.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/tunnel.example.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }
}
```

**DNS:** wildcard A record pointing to your VPS:

```
*.tunnel.example.com.  A  <your-vps-ip>
```

## CLI reference

### `tnnl http <PORT>`


| Flag             | Default       | Description                                       |
| ---------------- | ------------- | ------------------------------------------------- |
| `--to`           | `tnnl.run`    | Server address                                    |
| `--token`        | config / none | Shared secret for authentication                  |
| `--subdomain`    | random        | Request a specific subdomain                      |
| `--auth`         | none          | Protect tunnel with HTTP basic auth (`user:pass`) |
| `--control-port` | `9443`        | Server control port                               |
| `--inspect`      | off           | Print full request/response headers and body      |


### `tnnl replay <id>`

Re-send a captured request to your local server. IDs are shown in the request log.

### `tnnl server`

See [Self-hosting](#self-hosting) above.

## Why tnnl


|                        | tnnl | ngrok   | bore | frp      |
| ---------------------- | ---- | ------- | ---- | -------- |
| Self-hosted            | ✓    | ✗       | ✓    | ✓        |
| No account required    | ✓    | ✗       | ✓    | ✓        |
| Public shared server   | ✓    | ✓       | ✓    | ✗        |
| Single binary          | ✓    | ✓       | ✓    | ✗        |
| HTTP subdomain routing | ✓    | ✓       | ✗    | ✓        |
| Auth                   | HMAC | HMAC    | HMAC | token    |
| Config file            | ✓    | ✓       | ✗    | required |
| Auto-reconnect         | ✓    | ✓       | ✗    | ✓        |
| Request inspection     | ✓    | ✓       | ✗    | ✗        |
| Replay                 | ✓    | ✓       | ✗    | ✗        |
| Tunnel basic auth      | ✓    | paid    | ✗    | ✗        |
| Free                   | ✓    | limited | ✓    | ✓        |


## License

MIT