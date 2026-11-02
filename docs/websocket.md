# WebSocket multiplayer transport

Horizon clients can join a Horizon server over a binary WebSocket (`ws://` or `wss://`) instead of a raw
Minecraft TCP connection, and every Horizon server that accepts players also accepts them on a plaintext
WebSocket listener on loopback. The point is relays that only carry HTTP and WebSockets: a
[Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/) in
particular, which lets you host a server (or an Open-to-LAN world) from behind a home router or CGNAT with no
port forwarding, no public IP and no TLS certificate of your own. This page says how it works, how to set it
up on both sides and what it does not do yet. It is a fork feature (branch `websocket` of
[whos-carmen/horizon](https://github.com/whos-carmen/horizon)); upstream Horizon and vanilla clients do not
have it.

## What you get

- **Any `ws://` or `wss://` address joins.** Direct Connect, Add Server and `horizon-ctl`'s connect take
  `wss://mc.example.com`, `ws://127.0.0.1:25580` or `wss://mc.example.com:8443/play`. The port defaults to
  443 for `wss://` and 80 for `ws://`. The server list pings such entries like any other.
- **A server can open a WebSocket door.** With `websocket_port` set in `server.toml` (off by default),
  `horizon-server` also listens on `ws://127.0.0.1:<port>` next to the normal port; a world opened to the
  LAN does the same with `HORIZON_WS_PORT`. Only loopback: it is meant for a tunnel daemon on the same
  machine, which terminates TLS and forwards the plaintext WebSocket. Players who come through it are
  counted, limited, logged and bannable like players on the TCP port, under the address the tunnel reports.
- **Everything above the socket is unchanged.** Handshake, login, encryption in online mode, compression,
  configuration, play, secure chat: the Minecraft packet stream is the same bytes, carried in WebSocket binary
  frames instead of straight TCP. Whitelist, ops, bans and `server.toml` apply as they do to TCP players.
- **F3 says how you are connected.** A `net` group under the server line shows the address as joined, the
  transport (`memory`, `tcp`, `ws`, `wss`) and the path (`integrated`, `lan`, `cloudflare`, `tunnel`,
  `direct`), plus the TCP peer for a WebSocket, which is the Cloudflare edge for `wss://`.

## Setting it up

### The server

One setting. In `server.toml`:

```toml
[server]
websocket_port = 25580       # loopback port where a tunnel on this computer (cloudflared) hands players over a WebSocket; 0: off
```

(or `horizon-server config set server.websocket_port=25580`). The server writes it to `server.properties` as
`horizon-websocket-port`, which is what the game reads, so a vanilla-style folder can set that line directly.
When the server starts accepting players it also binds `127.0.0.1:<port>`, and the log says so:

```
[lan] WebSocket listener on ws://127.0.0.1:25580
```

A port it cannot take fails the start with the reason, like the main port would, so nothing runs half open.
Pick another port when 25580 is taken (a shared host with per-user port ranges, two servers on one machine).
`HORIZON_NET_DEBUG=1` additionally logs every WebSocket handshake that fails.

**Open to LAN** has no `server.properties`: set `HORIZON_WS_PORT` in the environment the game is started from
and the integrated server opens the same door when you publish the world, so a quick tunnel (below) shares a
singleplayer world across the internet. Unset, the door stays shut.

**Who can join is the same question as on the TCP port.** A tunnel makes the server reachable by anyone who
knows the address. In online mode (the default for `horizon-server`, and a LAN world whose host is signed
in) every player joins with their own account, and the whitelist applies. An offline server, or a LAN
world from a host who is not signed in, lets anyone join under any name: through a tunnel that is the whole
internet, not your living room. Read hosting.md's "Before you start" before you publish an address.

### The tunnel: Cloudflare

Install [`cloudflared`](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/)
on the machine that runs the server. The fastest way, no account or domain, is a quick tunnel:

```sh
cloudflared tunnel --url http://localhost:25580
```

It prints a hostname like `https://quiet-river-1234.trycloudflare.com`. Players join at
`wss://quiet-river-1234.trycloudflare.com`. The hostname changes every time you start it, and Cloudflare
gives no uptime promise for quick tunnels; they are for a session with friends, not a public server.

For a stable name you need a domain on Cloudflare (a free plan is enough) and a named tunnel:

```sh
cloudflared tunnel login                       # once: picks the zone in the browser
cloudflared tunnel create horizon              # once: writes the tunnel's credentials file
cloudflared tunnel route dns horizon mc.example.com
```

`~/.cloudflared/config.yml`:

```yaml
tunnel: horizon
credentials-file: /home/you/.cloudflared/<tunnel-id>.json
ingress:
  - hostname: mc.example.com
    service: http://localhost:25580
  - service: http_status:404
```

Then `cloudflared tunnel run horizon` (or install it as a service with `cloudflared service install`).
Players join at `wss://mc.example.com`. The dashboard's WebSockets setting (Network) is on by default; if a
handshake fails with a 4xx from Cloudflare, check it. `cloudflared` upgrades the connection and forwards the
plaintext WebSocket to the game, which is why the game's listener is loopback-only and never speaks TLS
itself.

The transport is shaped for Cloudflare's edge, which is why those numbers are in the code rather than in a
setting:

| What | Value | Why |
|---|---|---|
| Frame size | 64 KiB | The edge buffers frames up to 64 KiB; larger packets (chunks) are split into several frames and reassembled as a byte stream on the other side |
| Send budget | 2 MiB/s per connection, each direction | Keeps a world-join's chunk burst inside what the free tunnel passes without stalling |
| Keepalive | a ping every 30 s when nothing is being read | The edge closes idle WebSockets; the ping keeps the connection open while a player stands still |

And the limits that protect the server from a peer that is not a Horizon client, since a tunnel lets anyone
reach the door:

| What | Value | Why |
|---|---|---|
| Handshake deadline | 10 s, the login phase's | A socket that never finishes its upgrade request is dropped instead of holding a thread |
| Largest message | one Minecraft frame (2 MiB) | tungstenite would otherwise buffer up to 64 MiB per connection before the game's own packet limit saw a byte |
| Read backlog | 4 MiB | Past this the actor stops reading until the game catches up, so a flood waits in the socket, as it would on TCP |
| Write buffer | 1 MiB | Frames kept for a peer that reads slowly before the actor waits |
| Connections | the TCP door's count | Both doors share one accept loop: the player limit plus 64 spare connections, and the flood handling, apply to the sum |

### Other relays

Anything that terminates TLS and forwards a WebSocket upgrade works the same way: Caddy or nginx with a
Let's Encrypt certificate proxying `wss://mc.example.com` to `ws://127.0.0.1:25580`, Tailscale Funnel,
ngrok. The client checks `wss://` certificates against the Mozilla root store (rustls with webpki roots), so a
self-signed certificate is refused; use a real one or `ws://` inside a tailnet. F3 reports a `wss://`
hostname as `cloudflare` and a `wss://` IP address as `tunnel`; the label is a guess from the address, not a
probe.

### The client

Nothing to install. Join with the address in the server list or Direct Connect. In F3 you should see, for
example:

```
"Horizon" server, 12 tx, 40 rx
  net
    wss://mc.example.com
    wss · cloudflare · edge 104.21.1.2:443
```

`ws://` to a loopback address shows `ws · tunnel` (the tunnel daemon is on this machine), `ws://` to a
private address `ws · lan`, and anything else `direct`. A plain `host:port` keeps showing `tcp`.

## How it works

The code is small and lives in three places.

**`horizon-protocol`, `src/websocket.rs`: a synchronous byte stream over `tungstenite`.** Horizon's protocol
crate is blocking and splits every connection into a reader and a writer that run on different threads.
`tungstenite`'s `WebSocket` needs `&mut self` for both directions, so one actor thread per connection owns
it. The actor loop:

1. takes the next flushed batch of bytes off an outgoing channel and cuts it into frames of at most 64 KiB;
2. writes frames while under the 2 MiB/s budget (a one-second window), and sleeps 10 ms when over it;
3. flushes, and then acknowledges to the sender every batch whose frames have all reached `tungstenite`'s
   write buffer;
4. reads one message, non-blocking: binary frames go onto an incoming channel, a close frame ends the actor,
   and when nothing is readable it sends a ping if 30 s have passed and sleeps 1 ms.

The `WsStream` type is an `Arc` over the two channels and a raw clone of the TCP socket. Cloning it gives the
reader half and the writer half that `Connection` needs; both are cheap. `Read` drains the incoming channel
into a buffer and hands out bytes, so a packet that arrived as four frames reads back as one contiguous
stream, and it honours `set_read_timeout` with `recv_timeout`. `Write` appends to a pending buffer and
`flush` sends it to the actor as one batch, then waits up to 30 s for the acknowledgement; a timeout surfaces
as a broken pipe, which the game treats as a lost connection. `shutdown` (and drop) sets a stop flag and
shuts the raw TCP socket, which also wakes the actor out of a blocked write.

`WsStream::connect(url)` is the client side (`tungstenite::connect`, TLS through rustls for `wss://`), and
`WsStream::accept(TcpStream)` the server side (`tungstenite::accept` on a socket a listener handed over).

**`horizon-protocol`, `src/connection.rs` and `src/servers.rs`: the transport as a third stream kind.** The
private `Stream` enum became the public `ConnectionStream` with `Tcp`, `WebSocket` and `Memory` variants, and
every method on it (`try_clone`, `set_read_timeout`, `shutdown`, `peer_addr`, `Read`, `Write`) dispatches to
the WebSocket half. `Connection::connect`, `connect_server` and `connect_server_with_address` check
`is_websocket_address` first and go to `connect_websocket`, which fills in the scheme's default port and skips
the SRV lookup (a URL is used as written). `Connection::from_websocket` is the server side.
`ConnWriter::send` now flushes after every packet, which the TCP and memory streams ignore and the WebSocket
needs. `parse_address` recognises the two schemes, returns the whole URL as the host and the port from the
URL or the scheme, so the Add Server screen and the server list ping accept and use them unchanged.

**`horizon-client`, `src/lan/mod.rs`, `src/lan/conn.rs`, `src/net.rs`, `src/game/debug_keys.rs`: the
listener and F3.** `NetConfig::websocket_port` comes from `horizon-websocket-port` on a dedicated server
(`dedicated.rs`) and from `HORIZON_WS_PORT` on a LAN world. When it is set, `Lan::listen_on` binds
`127.0.0.1:<port>` before it starts the TCP accept thread, so a port it cannot take fails the start cleanly.
Both ports run the same `accept_loop`, each with its `Door` (thread name, log tag, serve function): every
accepted socket is counted against the player limit plus the spare connections, registered as waiting, and
turned away past the limit, whichever door it came through. `conn::serve_websocket` completes the WebSocket
handshake (`Connection::from_websocket`, with the login phase's 10 s deadline on the socket), replaces the
socket's loopback address with the one the tunnel reports in `CF-Connecting-IP` or `X-Forwarded-For` (the
same substitution Velocity forwarding makes, trusted only because a loopback listener's only peer is a
process on this computer), and then calls the same `serve_connection` that TCP players go through, so from
the handshake packet on there is one code path, and IP bans, the connection log and the per-address count
see the player. `Lan::stop` stops the WebSocket listener with the TCP one. On the client, `net.rs` carries a `NetInfo` on the
session: once the socket is up, `NetInfo::classify` labels the join address, and the F3 screen renders it as
the `net` group. Loopback, RFC 1918, link-local, CGNAT (which is what Tailscale uses) and unique-local IPv6
count as `lan`.

The dependency is `tungstenite` with `handshake` and `rustls-tls-webpki-roots` only, no async runtime.
Because the launcher workspace locks the game crates it builds against, the dependency is in both
`Cargo.lock` and `launcher/Cargo.lock`; a version bump has to land in both or `--locked` launcher checks fail.

## Checking it

```sh
cargo test -p horizon-protocol websocket
```

runs the unit tests in `websocket.rs` (a 200 000-byte request across many 64 KiB frames and a reply; a
socket that never finishes its handshake is dropped within the deadline; a message past the limit closes
the connection; the forwarded address comes from the proxy headers) and `tests/websocket_transport_tests.rs`
(a Minecraft-framed packet larger than one frame through `Connection::from_websocket` and
`connect_websocket`, plus the address check). The host's side is in `lan/mod.rs`'s tests
(`cargo test -p horizon-client-lib --lib websocket_door`): a flood on the WebSocket door is turned away at
the shared limit, and a status request through it is answered. To try the whole path on one machine without
Cloudflare: set `websocket_port`, start the server, join at `ws://127.0.0.1:25580`, and read F3.

## What it does not do yet

- **The per-address share is counted on the proxy's address.** A socket is counted when it is accepted,
  before the handshake reveals the player's address, so the ten-places-an-address rule sees loopback (which
  is exempt) and not the player; only the total limit holds for tunnel players. IP bans and the log do see
  the player.
- **The forwarded address is trusted from loopback.** Any process on the server's computer can connect to
  the door and claim an address. The listener never binds elsewhere, so that is the host's own processes.
- **No LAN screen setting.** A LAN world opens the door from `HORIZON_WS_PORT` only; there is no checkbox in
  the Open to LAN screen yet.
- **No TLS in the game.** `wss://` is only a client-side scheme. A server exposed directly to the internet
  over WebSocket needs a proxy or a tunnel in front of it; the game will not bind a public `wss://` listener.
- **Only Horizon clients from this fork.** Vanilla and upstream Horizon clients cannot use the WebSocket door;
  they keep joining on the TCP port.
- **No SRV, no proxy forwarding over WebSocket.** A `wss://` address is dialled as written. The Velocity
  forwarding path is untested through the WebSocket listener.
- **A busy connection is not pinged.** The keepalive only fires when a read would block; with constant
  traffic that is not needed, but a connection that only sends and never receives for 30 s or more relies on
  the edge's tolerance.
- **The budget and frame size are constants** (`MAX_FRAME`, `BYTES_PER_SECOND`, `PING_INTERVAL` in
  `websocket.rs`), chosen for Cloudflare. A relay with different limits means changing the code.
- **F3's path is a guess from the address.** `cloudflare` means "a `wss://` hostname", not a confirmed
  Cloudflare edge.
