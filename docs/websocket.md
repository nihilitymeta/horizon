# Play over a WebSocket

A Horizon client can join a server at a `ws://` or `wss://` address, and every Horizon server takes those
players on its normal game port. Use it to put a server behind a tunnel or a proxy that carries only HTTP,
such as a Cloudflare Tunnel: you host from home, behind CGNAT, with no port forwarding and no certificate of
your own.

Vanilla clients can't join over a WebSocket. They join the same server over TCP as before.

## How it works

- Minecraft's own bytes travel in WebSocket binary messages. Nothing above the connection changes: login,
  encryption in online mode, compression, the whitelist, bans and `server.toml` apply as they do over TCP.
- The server tells the two apart by the first bytes. A connection that opens with an HTTP request is a
  WebSocket; anything else is Minecraft over TCP. Both use the same port.
- Any path is served, so `wss://mc.example.com/` and `wss://mc.example.com/play` reach the same server.
- A plain web request (a browser, a health check) gets `426 Upgrade Required`.

## Join a server

Type the address in Direct Connect or Add Server:

| Address | Port used |
| --- | --- |
| `wss://mc.example.com` | 443 |
| `wss://mc.example.com:8443/play` | 8443 |
| `ws://192.168.1.20:25565` | 25565 |

`wss://` checks the server's certificate against the same public roots as a browser. An address with an
expired or self-signed certificate is refused.

## Host a server

The WebSocket door is on by default. In `server.toml`:

```toml
[server]
websocket = true                   # false: TCP players only
trusted_proxies = "127.0.0.1, ::1" # who may name the player's address
```

A world opened to LAN takes WebSocket players too, and trusts this computer only.

### The player's address

Through a proxy, every player seems to come from the proxy. A proxy listed in `trusted_proxies` can name the
real address in its handshake, and the server uses that address for bans, `log-ips` and the login line.

The server reads, in order:

1. `CF-Connecting-IP` (Cloudflare).
2. `Forwarded` or `X-Forwarded-For`: the nearest address that isn't a trusted proxy. Addresses further back
   were written by the player and are ignored.
3. `X-Real-IP` (nginx).

List addresses or networks, separated by commas: `"127.0.0.1, ::1, 10.0.0.0/8"`. A proxy that isn't listed
can't name an address, so a player can't claim one. An empty list trusts nobody.

### Cloudflare Tunnel

Install [`cloudflared`](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/)
on the computer that runs the server.

For a session with friends, start a quick tunnel. It needs no account:

```sh
cloudflared tunnel --url http://localhost:25565
```

It prints a name like `https://quiet-river-1234.trycloudflare.com`. Players join
`wss://quiet-river-1234.trycloudflare.com`. The name changes each time you start it.

For a name that lasts, you need a domain on Cloudflare (the free plan works):

1. Create a tunnel in the Cloudflare dashboard, under **Networks → Tunnels**, and run the connector it shows.
2. Add a public hostname, such as `mc.example.com`, with the service `http://localhost:25565`.
3. Players join `wss://mc.example.com`.

`cloudflared` runs on the same computer, so the default `trusted_proxies` already trusts it.

### nginx, Caddy and other proxies

Point the proxy at the game port and pass the upgrade on. For nginx:

```nginx
location / {
    proxy_pass http://127.0.0.1:25565;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_read_timeout 1h;
}
```

Caddy passes WebSockets on by itself: `reverse_proxy 127.0.0.1:25565`.

If the proxy runs on another computer, add its address to `trusted_proxies`.

## Use other WebSocket tools

The connection is a plain byte stream in binary messages, so general tools work:

- **Clients:** any library that sends binary messages, browsers included. Text messages are refused. How a
  client splits the stream into messages doesn't matter.
- **Servers:** a Horizon client joins any WebSocket server that passes the stream to a Minecraft server,
  such as `websocat -b ws-l:0.0.0.0:8080 tcp:127.0.0.1:25565`.
- **Subprotocols:** when a client offers some, the server answers with the first. Browsers need an answer
  to connect.

## Limits

- The server doesn't serve TLS itself. Put `wss://` in front with a tunnel or a proxy.
- Compression (`permessage-deflate`) isn't offered. Minecraft compresses its packets already.
- A tunnel adds a hop. Expect a few milliseconds more ping than a direct connection.
