# DroidTransfer protocol v1

Contract between the PC app (Rust, `pc/`) and the Android app (Kotlin, `android/`).
Both implementations MUST follow this file exactly. All integers big-endian.

## Ports

| Purpose | Port |
|---|---|
| UDP discovery (both sides bind `0.0.0.0:47820`, SO_REUSEADDR + SO_BROADCAST) | 47820 |
| PC TCP server | 47821 |
| Phone TCP server (runs inside the Android foreground service) | 47822 |

## Identity

- Each install has a random `id` (UUID v4 string, lowercase) generated on first run and persisted.
- `name`: human readable (PC: computer hostname by default; phone: `Build.MANUFACTURER + " " + Build.MODEL`).
- `kind`: `"pc"` or `"phone"`.

## Discovery (UDP, JSON UTF-8, one datagram per message, max 1024 bytes)

```json
{"t":"disc","v":1,"id":"<uuid>","name":"<name>","kind":"pc","port":47821}
{"t":"here","v":1,"id":"<uuid>","name":"<name>","kind":"phone","port":47822}
```

- `disc` is broadcast to `255.255.255.255:47820` AND to the directed broadcast address of every
  up, non-loopback IPv4 interface (e.g. `192.168.1.255:47820`).
- On receiving a `disc` whose `kind` differs from your own: reply with `here` unicast to the
  packet's source address/port. Ignore packets with your own `id`, unknown `t`, or `v != 1`.
- On receiving either `disc` or `here` from the other kind: record/refresh the peer
  `{id, name, ip = packet source IP, port, lastSeen = now}`.
- PC broadcasts `disc` every 5 s. Phone broadcasts `disc` every 3 s while its main screen is
  visible, plus once when the service starts. The phone service always answers `disc` with `here`.
- A peer is "online" if `lastSeen` is within the last 15 s.
- The PC also records the phone's IP from every accepted TCP connection (persist as `lastIp`).
  The phone records the PC IP likewise.

## TCP framing

Every message is a frame:

```
u8  type
u32 length      (payload byte count, <= 16 MiB; reject larger -> close)
[length] payload
```

JSON payloads are UTF-8 objects. Unknown JSON fields are ignored.

| Type | Name | Payload | Direction |
|---|---|---|---|
| 0x01 | HELLO | `{"v":1,"id","name","kind","nonce":"<32 hex>"}` | client -> server |
| 0x02 | HELLO_ACK | `{"v":1,"id","name","kind","nonce":"<32 hex>","paired":bool}` | server -> client |
| 0x03 | AUTH | `{"mac":"<64 hex>"}` | client -> server |
| 0x04 | AUTH_OK | `{"mac":"<64 hex>"}` | server -> client |
| 0x05 | ERROR | `{"msg":"<text>"}` | either; sender closes after |
| 0x06 | PAIR_REQ | `{}` | client -> server (only phone -> PC) |
| 0x07 | PAIR_OK | `{"secret":"<64 hex>"}` | server -> client |
| 0x10 | TEXT | `{"text":"<utf8>"}` | client -> server |
| 0x11 | TEXT_ACK | `{}` | server -> client |
| 0x20 | BATCH_START | `{"count":N,"totalBytes":T}` (T = sum of known sizes) | client -> server |
| 0x21 | FILE_START | `{"path":"dir/sub/a.jpg","size":S,"mtime":ms}` (S = -1 if unknown, mtime 0 if unknown) | client -> server |
| 0x22 | DATA | raw bytes, 1..1048576 bytes per frame | client -> server |
| 0x23 | FILE_END | `{}` | client -> server |
| 0x24 | BATCH_END | `{}` | client -> server |
| 0x25 | BATCH_ACK | `{"ok":true,"saved":N}` | server -> client |

Hex is lowercase. `nonce` = 16 random bytes (SecureRandom / OsRng).

## Handshake

1. Client connects, sends HELLO. Server replies HELLO_ACK; `paired` = server has a stored secret
   for the client's `id`.
2. If paired (and client also has a secret for the server's `id`):
   - `key` = the 32 raw secret bytes.
   - client sends AUTH with `mac = HMAC-SHA256(key, "DT1-C" || cNonce || sNonce)` where nonces are
     the 16 RAW bytes (not hex) and `"DT1-C"` is the 5 ASCII bytes.
   - server verifies (constant-time compare). On failure: ERROR `{"msg":"auth failed"}`, close.
   - server sends AUTH_OK with `mac = HMAC-SHA256(key, "DT1-S" || cNonce || sNonce)`.
   - client verifies; on failure closes. Connection is now authenticated.
3. If not paired: only the PC server accepts pairing. The phone sends PAIR_REQ.
   - Both sides compute the 6-digit verification code:
     `code = (u32 big-endian of first 4 bytes of SHA-256(cNonce || sNonce)) % 1000000`,
     shown zero-padded to 6 digits on both screens.
   - PC shows "Pair with <name> (<ip>)? Code 123456" with Accept / Reject, timeout 60 s.
   - Accept: PC generates 32 random bytes, stores `{id, name, secret, lastIp}`, sends PAIR_OK.
     Phone stores `{id, name, secret, lastIp}`. Both close the connection.
   - Reject / timeout: ERROR `{"msg":"rejected"}` or `{"msg":"timeout"}`, close.
   - A phone server receiving PAIR_REQ, or any unauthenticated non-pairing frame: ERROR, close.
4. One authenticated connection carries exactly ONE operation (a TEXT or a batch), then the client
   closes after receiving the ack. A new operation opens a new connection.

## Text

Client sends TEXT, server replies TEXT_ACK. Receiver puts the text on its clipboard and records
it in the UI history (phone: also posts a notification with a "Copy" action).

## File batch

```
BATCH_START
  ( FILE_START  DATA*  FILE_END ) * count
BATCH_END
                    <- BATCH_ACK
```

- `path` is relative with `/` separators. Single files: just the file name. Folders: the folder
  name plus inner path (`Photos/2024/a.jpg`). Empty folders are not transferred.
- Receiver sanitizes every path: split on `/` and `\`, drop empty, `.` segments; reject the file
  (ERROR, close) if any segment is `..`, or contains `:` or a NUL; strip characters invalid on the
  target filesystem (`<>:"|?*` and control chars on Windows) replacing them with `_`.
- Name collisions: if `a.jpg` exists, use `a (1).jpg`, `a (2).jpg`, ... (applies only to the file,
  not to folders: folders merge).
- If `size >= 0` and the received byte count differs, the receiver deletes the partial file and
  sends ERROR. On any error/disconnect the receiver deletes the partial file being written.
- Sender cancellation = closing the socket.
- Destinations: PC -> configured download dir (default `%USERPROFILE%\Downloads\DroidTransfer`).
  Phone -> `MediaStore.Downloads` with `RELATIVE_PATH = "Download/DroidTransfer/<subdirs>/"`.
- Performance: use >= 1 MiB read buffers, DATA frames of up to 1 MiB, buffered socket streams,
  socket send/receive buffers set to 4 MiB where allowed.

## Security notes

- Only paired devices can send or receive. Pairing needs a click on the PC plus matching codes.
- v1 does not encrypt the payload (it relies on the Wi-Fi link encryption). Auth is mutual HMAC.
