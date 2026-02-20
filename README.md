# tnnl

Expose localhost to the internet. Single binary, no account required.

![demo](https://vhs.charm.sh/vhs-7x43LFVT4jrZOnRC8oNBJn.gif)

## Quickstart

Use the public shared server at [tnnl.run](https://tnnl.run):

```sh
tnnl http 3000 --to tnnl.run
# → abc12345.tnnl.run is live
```

Or self-host on your own VPS:

```sh
# On your VPS (once)
tnnl server --domain tunnel.example.com

# On your laptop
tnnl http 3000 --to tunnel.example.com
# → abc12345.tunnel.example.com is live
```

Every request gets logged:

```
00:05  #1  GET    /                          200  16ms
00:09  #2  GET    /api/users                 200  20ms
00:14  #3  POST   /api/webhooks              201  14ms
```

## Install

**One-line install** (Linux / macOS):

```sh
curl -fsSL https://tnnl.run/install.sh | sh
```

**From releases**: grab a prebuilt binary from the [releases page](https://github.com/soria-dev/tnnl/releases).

**From source** (requires Rust):

```sh
cargo install --path .
```

## Config file

Put your settings in `~/.tnnl.toml` so you don't have to pass flags every time:

```toml
server = "tnnl.run"
token  = "your-secret"     # omit if using an open server
subdomain = "myapp"        # optional - get the same URL every time
inspect = false            # optional
auth = "user:pass"         # optional - HTTP basic auth on the tunnel
```

Then just run:

```sh
tnnl http 3000
```

CLI flags always override the config file.

## Debugging webhooks

### Inspect mode

Pass `--inspect` to see full headers and body for every request:

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

### Replay

Every request is saved locally. Fix your handler, then replay instead of going back to Stripe's dashboard:

```sh
tnnl replay 3
```

Request IDs are shown in the log and persist across restarts.

## Protecting your tunnel

Add HTTP basic auth to the exposed URL - unauthenticated requests get a 401 before they reach your local service:

```sh
tnnl http 3000 --auth user:pass
```

Or set it in `~/.tnnl.toml`:

```toml
auth = "user:pass"
```

## How it works

tnnl is two modes in one binary: `tnnl server` runs on a VPS, `tnnl http` runs on your machine.

1. Client connects to the server over TCP and authenticates using HMAC-SHA256 - the secret never travels over the wire
2. Server assigns a subdomain (random or requested with `--subdomain`)
3. When HTTP traffic hits `*.tunnel.example.com`, the server routes it via the `Host` header to the right client
4. Traffic flows over a single multiplexed connection ([yamux](https://github.com/libp2p/rust-yamux)) - no new TCP handshake per request
5. Client forwards to localhost and pipes the response back

If localhost isn't running, the browser gets a clean `502 Bad Gateway`. If the connection drops, the client reconnects with exponential backoff.

## Self-hosting

### Server flags


| Flag             | Default  | Description                                     |
| ---------------- | -------- | ----------------------------------------------- |
| `--domain`       | required | Base domain for tunnel subdomains               |
| `--token`        | none     | Secret clients must know — omit for open server |
| `--control-port` | 9443     | Port for client connections                     |
| `--http-port`    | 8080     | Port for public HTTP traffic                    |


Omitting `--token` makes it an open server (anyone can connect). The server has built-in abuse protection regardless: per-IP rate limiting, per-IP tunnel caps, and a global tunnel limit.

### TLS

tnnl doesn't handle TLS itself. Put it behind Caddy or nginx with a wildcard cert.

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

**DNS:** Add a wildcard A record pointing to your VPS:

```
*.tunnel.example.com.  A  <your-vps-ip>
```

## CLI reference

### `tnnl http <PORT>`


| Flag             | Default       | Description                                       |
| ---------------- | ------------- | ------------------------------------------------- |
| `--to`           | `127.0.0.1`   | Server address                                    |
| `--token`        | config / none | Shared secret for authentication                  |
| `--subdomain`    | random        | Request a specific subdomain                      |
| `--auth`         | none          | Protect tunnel with HTTP basic auth (`user:pass`) |
| `--control-port` | 9443          | Server control port                               |
| `--inspect`      | off           | Print full request/response headers and body      |


### `tnnl replay <id>`

Re-send a previously captured request to your local server. IDs are shown in the request log.

### `tnnl server`

See [Self-hosting](#self-hosting) above.

## Why tnnl


|                        | tnnl    | ngrok   | bore | frp      |
| ---------------------- | ------- | ------- | ---- | -------- |
| Self-hosted            | yes     | no      | yes  | yes      |
| Public shared server   | yes     | yes     | yes  | no       |
| Single binary          | yes     | yes     | yes  | no       |
| HTTP subdomain routing | yes     | yes     | no   | yes      |
| Auth                   | HMAC    | HMAC    | HMAC | token    |
| Config file            | yes     | yes     | no   | required |
| Auto-reconnect         | yes     | yes     | no   | yes      |
| Request inspection     | yes     | paid    | no   | no       |
| Replay                 | yes     | paid    | no   | no       |
| Tunnel basic auth      | yes     | paid    | no   | no       |
| Free                   | yes     | limited | yes  | yes      |


## License

MIT