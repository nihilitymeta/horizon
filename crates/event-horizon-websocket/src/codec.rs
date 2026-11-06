//! RFC 6455 frames, with no I/O of its own: [`encode`] appends a frame to a buffer, and a [`Decoder`] turns
//! the bytes a peer sent into the payload of its binary messages, answering its pings and its close.
//!
//! A connection carries Minecraft's own byte stream, so how a peer splits it into messages and frames
//! doesn't matter: payloads are passed on as they arrive, unmasked, and a frame is never held whole.

pub const CONTINUATION: u8 = 0x0;
pub const TEXT: u8 = 0x1;
pub const BINARY: u8 = 0x2;
pub const CLOSE: u8 = 0x8;
pub const PING: u8 = 0x9;
pub const PONG: u8 = 0xa;

/// The most payload an outgoing frame carries. Proxies pass any size, but Cloudflare's edge buffers whole
/// frames and handles ones of this size best; a larger write becomes several messages.
pub const MAX_OUT_FRAME: usize = 64 * 1024;

/// Close codes (RFC 6455, section 7.4.1).
pub const NORMAL: u16 = 1000;
pub const PROTOCOL_ERROR: u16 = 1002;
pub const UNSUPPORTED_DATA: u16 = 1003;

/// Which end of the connection this side is: a client masks what it sends, a server must not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Client,
    Server,
}

/// Appends a frame of `opcode` carrying `payload`, masked when `role` is the client's.
pub fn encode(out: &mut Vec<u8>, opcode: u8, payload: &[u8], role: Role) {
    let masked = if role == Role::Client { 0x80 } else { 0 };
    out.push(0x80 | opcode);
    match payload.len() {
        n @ 0..=125 => out.push(masked | n as u8),
        n @ 126..=0xffff => {
            out.push(masked | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(masked | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    if role == Role::Server {
        out.extend_from_slice(payload);
        return;
    }
    let mask = mask();
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
}

/// Appends `data` as binary messages of at most [`MAX_OUT_FRAME`] bytes each.
pub fn encode_binary(out: &mut Vec<u8>, data: &[u8], role: Role) {
    for chunk in data.chunks(MAX_OUT_FRAME) {
        encode(out, BINARY, chunk, role);
    }
}

/// Appends a close frame with `code`.
pub fn encode_close(out: &mut Vec<u8>, code: u16, role: Role) {
    encode(out, CLOSE, &code.to_be_bytes(), role);
}

/// A client's masking key: from the OS's randomness, as section 5.3 asks, so that a peer can't choose
/// the bytes a proxy on the way sees.
fn mask() -> [u8; 4] {
    let mut mask = [0u8; 4];
    if getrandom::fill(&mut mask).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        mask = nanos.to_le_bytes();
    }
    mask
}

/// Why a peer's frames were refused, and the close code that says so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub code: u16,
    pub reason: &'static str,
}
impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WebSocket: {} (close code {})", self.reason, self.code)
    }
}
impl std::error::Error for Refused {}

fn refused(code: u16, reason: &'static str) -> Refused {
    Refused { code, reason }
}

/// The frame whose payload is arriving.
struct Frame {
    opcode: u8,
    fin: bool,
    left: u64,
    mask: Option<[u8; 4]>,
    /// Payload bytes unmasked so far: where in the mask the next one starts.
    at: usize,
}

/// Reads a peer's frames as their bytes arrive.
pub struct Decoder {
    role: Role,
    /// The next frame's header, until it is whole (at most 14 bytes).
    head: Vec<u8>,
    frame: Option<Frame>,
    /// Inside a fragmented binary message: continuation frames are expected.
    in_message: bool,
    /// A control frame's payload (at most 125 bytes).
    control: Vec<u8>,
    closed: bool,
}

impl Decoder {
    pub fn new(role: Role) -> Self {
        Self {
            role,
            head: Vec::with_capacity(14),
            frame: None,
            in_message: false,
            control: Vec::new(),
            closed: false,
        }
    }

    /// This side's role, whose frames the replies are.
    pub fn role(&self) -> Role {
        self.role
    }

    /// Whether the peer closed the connection: nothing it sends after that counts.
    pub fn closed(&self) -> bool {
        self.closed
    }

    /// Takes in `raw`, the peer's bytes: binary payload goes to `data`, and the frames that answer the peer
    /// (a pong for each ping, the echo of its close) to `replies`.
    pub fn decode(
        &mut self,
        mut raw: &[u8],
        data: &mut Vec<u8>,
        replies: &mut Vec<u8>,
    ) -> Result<(), Refused> {
        while !raw.is_empty() && !self.closed {
            let Some(frame) = &mut self.frame else {
                let take = self.header_wanted().saturating_sub(self.head.len()).max(1);
                let take = take.min(raw.len());
                self.head.extend_from_slice(&raw[..take]);
                raw = &raw[take..];
                if self.head.len() == self.header_wanted() {
                    self.start_frame()?;
                    self.end_frame_if_done(replies);
                }
                continue;
            };
            let n = usize::try_from(frame.left)
                .unwrap_or(usize::MAX)
                .min(raw.len());
            let control = frame.opcode >= CLOSE;
            let into = if control {
                &mut self.control
            } else {
                &mut *data
            };
            let start = into.len();
            into.extend_from_slice(&raw[..n]);
            if let Some(mask) = frame.mask {
                for (i, b) in into[start..].iter_mut().enumerate() {
                    *b ^= mask[(frame.at + i) % 4];
                }
            }
            frame.at = (frame.at + n) % 4;
            frame.left -= n as u64;
            raw = &raw[n..];
            self.end_frame_if_done(replies);
        }
        Ok(())
    }

    /// How long the header in `head` is, from what it says so far.
    fn header_wanted(&self) -> usize {
        let Some(&second) = self.head.get(1) else {
            return 2;
        };
        let length = match second & 0x7f {
            126 => 2,
            127 => 8,
            _ => 0,
        };
        let mask = if second & 0x80 != 0 { 4 } else { 0 };
        2 + length + mask
    }

    /// Checks the whole header in `head` and starts its frame.
    fn start_frame(&mut self) -> Result<(), Refused> {
        let head = std::mem::take(&mut self.head);
        let (fin, rsv, opcode) = (head[0] & 0x80 != 0, head[0] & 0x70, head[0] & 0x0f);
        let masked = head[1] & 0x80 != 0;
        if rsv != 0 {
            return Err(refused(PROTOCOL_ERROR, "no extension was agreed"));
        }
        match (self.role, masked) {
            (Role::Server, false) => {
                return Err(refused(PROTOCOL_ERROR, "a client's frames must be masked"));
            }
            (Role::Client, true) => {
                return Err(refused(
                    PROTOCOL_ERROR,
                    "a server's frames must not be masked",
                ));
            }
            _ => {}
        }
        let (left, rest) = match head[1] & 0x7f {
            126 => (
                u64::from(u16::from_be_bytes([head[2], head[3]])),
                &head[4..],
            ),
            127 => {
                let length = u64::from_be_bytes(head[2..10].try_into().expect("8 bytes"));
                if length >> 63 != 0 {
                    return Err(refused(
                        PROTOCOL_ERROR,
                        "a frame's length has its top bit set",
                    ));
                }
                (length, &head[10..])
            }
            n => (u64::from(n), &head[2..]),
        };
        let mask: Option<[u8; 4]> = masked.then(|| rest[..4].try_into().expect("4 bytes"));
        match opcode {
            CLOSE | PING | PONG => {
                if !fin || left > 125 {
                    return Err(refused(
                        PROTOCOL_ERROR,
                        "a control frame is short and whole",
                    ));
                }
                self.control.clear();
            }
            TEXT => {
                return Err(refused(
                    UNSUPPORTED_DATA,
                    "Minecraft is sent in binary messages",
                ));
            }
            BINARY if self.in_message => {
                return Err(refused(PROTOCOL_ERROR, "a message started inside another"));
            }
            CONTINUATION if !self.in_message => {
                return Err(refused(
                    PROTOCOL_ERROR,
                    "a continuation frame with no message",
                ));
            }
            BINARY | CONTINUATION => {}
            _ => return Err(refused(PROTOCOL_ERROR, "unknown frame opcode")),
        }
        self.head = head;
        self.head.clear();
        self.frame = Some(Frame {
            opcode,
            fin,
            left,
            mask,
            at: 0,
        });
        Ok(())
    }

    /// Ends the frame once its payload is whole: a data frame says whether its message goes on, a control
    /// frame is answered.
    fn end_frame_if_done(&mut self, replies: &mut Vec<u8>) {
        let Some(frame) = self.frame.take_if(|f| f.left == 0) else {
            return;
        };
        match frame.opcode {
            PING => encode(replies, PONG, &self.control, self.role),
            PONG => {}
            CLOSE => {
                self.closed = true;
                // Its code, echoed (section 5.5.1); a close without one gets one without.
                let code = self.control.get(..2).unwrap_or_default();
                encode(replies, CLOSE, code, self.role);
            }
            _ => self.in_message = !frame.fin,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame as a peer in `role` sends it, with a fixed mask when masked.
    fn frame(fin: bool, opcode: u8, payload: &[u8], role: Role) -> Vec<u8> {
        let mask = [0x37, 0xfa, 0x21, 0x3d];
        let masked = role == Role::Client;
        let mut f = vec![if fin { 0x80 } else { 0 } | opcode];
        let bit = if masked { 0x80 } else { 0 };
        match payload.len() {
            n @ 0..=125 => f.push(bit | n as u8),
            n @ 126..=0xffff => {
                f.push(bit | 126);
                f.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                f.push(bit | 127);
                f.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        if masked {
            f.extend_from_slice(&mask);
            f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        } else {
            f.extend_from_slice(payload);
        }
        f
    }

    /// Feeds `raw` in pieces of `step` bytes: the data and the replies.
    fn decode(role: Role, raw: &[u8], step: usize) -> Result<(Vec<u8>, Vec<u8>), Refused> {
        let mut d = Decoder::new(role);
        let (mut data, mut replies) = (Vec::new(), Vec::new());
        for piece in raw.chunks(step) {
            d.decode(piece, &mut data, &mut replies)?;
        }
        Ok((data, replies))
    }

    #[test]
    fn rfc_examples_decode() {
        // RFC 6455, section 5.7: "Hello" unmasked from a server, masked from a client, and a ping.
        let unmasked = [0x82, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f];
        assert_eq!(decode(Role::Client, &unmasked, 100).unwrap().0, b"Hello");
        let masked = [
            0x82, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
        ];
        assert_eq!(decode(Role::Server, &masked, 100).unwrap().0, b"Hello");
        // Fragmented: "Hel" then "lo".
        let fragmented = [
            0x02, 0x03, 0x48, 0x65, 0x6c, //
            0x80, 0x02, 0x6c, 0x6f,
        ];
        assert_eq!(decode(Role::Client, &fragmented, 100).unwrap().0, b"Hello");
    }

    #[test]
    fn bytes_in_any_pieces_give_the_same_stream() {
        let payload: Vec<u8> = (0..70_000u32).map(|i| (i * 7) as u8).collect();
        let mut raw = frame(false, BINARY, &payload[..200], Role::Client);
        raw.extend(frame(true, PING, b"are you there", Role::Client));
        raw.extend(frame(
            false,
            CONTINUATION,
            &payload[200..66_000],
            Role::Client,
        ));
        raw.extend(frame(true, CONTINUATION, &payload[66_000..], Role::Client));
        raw.extend(frame(true, BINARY, b"", Role::Client));
        raw.extend(frame(true, BINARY, b"tail", Role::Client));
        let mut expected = payload.clone();
        expected.extend_from_slice(b"tail");
        for step in [1, 2, 3, 13, 125, 4096, raw.len()] {
            let (data, replies) = decode(Role::Server, &raw, step).unwrap();
            assert_eq!(data, expected, "in pieces of {step}");
            // The pong carries the ping's payload, unmasked from a server.
            let mut pong = vec![0x8a, 13];
            pong.extend_from_slice(b"are you there");
            assert_eq!(replies, pong, "in pieces of {step}");
        }
    }

    #[test]
    fn a_close_is_echoed_and_ends_the_stream() {
        let mut raw = frame(true, BINARY, b"last", Role::Server);
        raw.extend(frame(true, CLOSE, &1001u16.to_be_bytes(), Role::Server));
        raw.extend(frame(true, BINARY, b"after", Role::Server));
        let mut d = Decoder::new(Role::Client);
        let (mut data, mut replies) = (Vec::new(), Vec::new());
        d.decode(&raw, &mut data, &mut replies).unwrap();
        assert!(d.closed());
        assert_eq!(data, b"last");
        // A client's echo is masked: the opcode, the masked length, a mask, and 1001 under it.
        assert_eq!(&replies[..2], &[0x88, 0x82]);
        let mask = &replies[2..6];
        assert_eq!([replies[6] ^ mask[0], replies[7] ^ mask[1]], [0x03, 0xe9]);
    }

    #[test]
    fn what_rfc_6455_forbids_is_refused() {
        let cases: [(Role, Vec<u8>, u16); 7] = [
            // Unmasked from a client, masked from a server.
            (
                Role::Server,
                frame(true, BINARY, b"x", Role::Server),
                PROTOCOL_ERROR,
            ),
            (
                Role::Client,
                frame(true, BINARY, b"x", Role::Client),
                PROTOCOL_ERROR,
            ),
            (
                Role::Server,
                frame(true, TEXT, b"x", Role::Client),
                UNSUPPORTED_DATA,
            ),
            (
                Role::Server,
                frame(true, CONTINUATION, b"x", Role::Client),
                PROTOCOL_ERROR,
            ),
            (
                Role::Server,
                frame(false, PING, b"x", Role::Client),
                PROTOCOL_ERROR,
            ),
            (
                Role::Server,
                frame(true, 0x3, b"x", Role::Client),
                PROTOCOL_ERROR,
            ),
            // A compressed frame (RSV1) when no extension was agreed.
            (
                Role::Server,
                {
                    let mut f = frame(true, BINARY, b"x", Role::Client);
                    f[0] |= 0x40;
                    f
                },
                PROTOCOL_ERROR,
            ),
        ];
        for (role, raw, code) in cases {
            assert_eq!(
                decode(role, &raw, 100).unwrap_err().code,
                code,
                "{raw:02x?}"
            );
        }
        let mut nested = frame(false, BINARY, b"a", Role::Client);
        nested.extend(frame(true, BINARY, b"b", Role::Client));
        assert_eq!(
            decode(Role::Server, &nested, 100).unwrap_err().code,
            PROTOCOL_ERROR
        );
    }

    #[test]
    fn what_one_side_encodes_the_other_decodes() {
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        for (sender, receiver) in [(Role::Client, Role::Server), (Role::Server, Role::Client)] {
            let mut raw = Vec::new();
            encode_binary(&mut raw, &data, sender);
            assert_eq!(decode(receiver, &raw, 9000).unwrap().0, data);
        }
        // 200 000 bytes in 64 KiB frames: three full ones, with 64-bit lengths, and the rest.
        let mut raw = Vec::new();
        encode_binary(&mut raw, &data, Role::Server);
        assert_eq!(raw.len(), data.len() + 3 * 10 + 4);
    }
}
