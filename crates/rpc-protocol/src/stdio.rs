//! Wire framing for `RpcMessage` over a raw byte stream (a child process's
//! stdin/stdout, in practice — see `stdio_client`).
//!
//! One `serde_json::to_string(&message)` line, `\n`-terminated (JSON never
//! contains a raw, unescaped newline, so this needs no framing beyond what
//! `serde_json` already does). If that message's `binary_len()` is
//! `Some(n)`, exactly `n` raw bytes follow *immediately after* the newline,
//! completely unencoded (no base64, no further framing) — the reader knows
//! to expect them because it already parsed the length from the line it
//! just read, and reads exactly that many bytes via `Read::read_exact`
//! before resuming line-based reading for the next message. This is how a
//! byte pipe (which has no message boundaries of its own, unlike e.g. a
//! WebSocket frame) gets to carry raw binary payloads without an encoding
//! tax: the JSON line is self-describing, so the binary right after it
//! never needs its own delimiter.
use crate::RpcMessage;
use std::io::{BufRead, Write};

/// Reads one `RpcMessage` (and its binary attachment, if it declared one)
/// from `reader`. Returns `Ok(None)` on a clean EOF (the writer closed the
/// stream with nothing left to read) — distinct from an error, since EOF
/// here is the normal "the child process exited" case, not a protocol
/// violation.
pub fn read_message<R: BufRead>(reader: &mut R) -> std::io::Result<Option<(RpcMessage, Option<Vec<u8>>)>> {
    let mut line = String::new();
    let bytes_read = reader.read_line(&mut line)?;
    if bytes_read == 0 {
        return Ok(None);
    }
    let trimmed = line.trim_end_matches(['\n', '\r']);
    let message: RpcMessage = serde_json::from_str(trimmed)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("malformed rpc message line {trimmed:?}: {err}")))?;
    let binary = match message.binary_len() {
        Some(len) => {
            let mut buf = vec![0u8; len as usize];
            reader.read_exact(&mut buf)?;
            Some(buf)
        }
        None => None,
    };
    Ok(Some((message, binary)))
}

/// Writes one `RpcMessage` (and its binary attachment, if any) to `writer`,
/// flushing afterward — a stdio pipe to a child process is typically
/// unbuffered-by-default from the OS's point of view, but the `Write`
/// wrapper the caller hands in might not be, so this doesn't assume it.
///
/// Returns an error (not just a `debug_assert!`) if `binary`'s length
/// doesn't match `message.binary_len()`: sending a message that promises a
/// binary attachment of one length but then writing a different number of
/// bytes would desynchronize the reader's framing for every message after
/// it, silently, in a release build — this has to be caught here, not left
/// to corrupt the whole rest of the stream.
pub fn write_message<W: Write>(writer: &mut W, message: &RpcMessage, binary: Option<&[u8]>) -> std::io::Result<()> {
    let declared = message.binary_len();
    let actual = binary.map(|bytes| bytes.len() as u64);
    if declared != actual {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("message declares binary_len {declared:?} but {actual:?} bytes were given"),
        ));
    }
    let line = serde_json::to_string(message).map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    writer.write_all(line.as_bytes())?;
    writer.write_all(b"\n")?;
    if let Some(bytes) = binary {
        writer.write_all(bytes)?;
    }
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RpcError;
    use std::io::Cursor;

    fn request(id: u64, binary_len: Option<u64>) -> RpcMessage {
        RpcMessage::Request { id, method: "ping".to_string(), params: serde_json::json!({"n": 1}), binary_len }
    }

    #[test]
    fn round_trips_a_json_only_message() {
        let mut buf = Vec::new();
        let msg = request(1, None);
        write_message(&mut buf, &msg, None).unwrap();
        let mut cursor = Cursor::new(buf);
        let (read_back, binary) = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(read_back, msg);
        assert_eq!(binary, None);
    }

    #[test]
    fn round_trips_a_message_with_a_binary_attachment() {
        let mut buf = Vec::new();
        let payload = vec![0u8, 1, 2, 255, 254, b'\n', b'{', b'}'];
        let msg = request(2, Some(payload.len() as u64));
        write_message(&mut buf, &msg, Some(&payload)).unwrap();
        let mut cursor = Cursor::new(buf);
        let (read_back, binary) = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(read_back, msg);
        assert_eq!(binary, Some(payload));
    }

    #[test]
    fn writes_two_messages_back_to_back_correctly() {
        let mut buf = Vec::new();
        write_message(&mut buf, &request(1, Some(3)), Some(&[9, 9, 9])).unwrap();
        write_message(&mut buf, &request(2, None), None).unwrap();
        let mut cursor = Cursor::new(buf);
        let (first, first_binary) = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(first, request(1, Some(3)));
        assert_eq!(first_binary, Some(vec![9, 9, 9]));
        let (second, second_binary) = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(second, request(2, None));
        assert_eq!(second_binary, None);
    }

    #[test]
    fn write_message_rejects_a_binary_len_mismatch() {
        let mut buf = Vec::new();
        let msg = request(1, Some(5));
        let err = write_message(&mut buf, &msg, Some(&[1, 2, 3])).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn read_message_returns_none_on_clean_eof() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        assert_eq!(read_message(&mut cursor).unwrap(), None);
    }

    #[test]
    fn read_message_errors_on_a_malformed_line() {
        let mut cursor = Cursor::new(b"not json at all\n".to_vec());
        let err = read_message(&mut cursor).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn read_message_errors_on_a_truncated_binary_attachment() {
        let msg = request(1, Some(10));
        let mut buf = Vec::new();
        let line = serde_json::to_string(&msg).unwrap();
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
        buf.extend_from_slice(&[1, 2, 3]); // only 3 of the promised 10 bytes
        let mut cursor = Cursor::new(buf);
        let err = read_message(&mut cursor).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn round_trips_a_response_with_an_error() {
        let mut buf = Vec::new();
        let msg = RpcMessage::Response {
            id: 7,
            result: Err(RpcError { code: -1, message: "boom".to_string(), data: None }),
            binary_len: None,
        };
        write_message(&mut buf, &msg, None).unwrap();
        let mut cursor = Cursor::new(buf);
        let (read_back, binary) = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(read_back, msg);
        assert_eq!(binary, None);
    }

    #[test]
    fn round_trips_a_notification() {
        let mut buf = Vec::new();
        let msg = RpcMessage::Notification { method: "status".to_string(), params: serde_json::json!("ready"), binary_len: None };
        write_message(&mut buf, &msg, None).unwrap();
        let mut cursor = Cursor::new(buf);
        let (read_back, _) = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(read_back, msg);
    }
}
