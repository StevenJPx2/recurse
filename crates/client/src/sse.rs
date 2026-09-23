use recurse_protocol::{SseFrame, bounds};

use crate::ClientError;

/// An open `GET /events` stream, read one frame at a time.
pub struct EventStream {
    response: reqwest::Response,
    buffer: Vec<u8>,
}

impl EventStream {
    pub(crate) const fn new(response: reqwest::Response) -> Self {
        Self {
            response,
            buffer: Vec::new(),
        }
    }

    /// The next frame, or `None` when the daemon closed the stream.
    pub async fn next_frame(&mut self) -> Result<Option<SseFrame>, ClientError> {
        loop {
            if let Some(block) = self.take_block() {
                if let Some(frame) = parse_block(&block)? {
                    return Ok(Some(frame));
                }

                continue;
            }

            if self.buffer.len() > bounds::SSE_FRAME.saturating_mul(2) {
                return Err(ClientError::Protocol("SSE frame exceeds 1 MiB".into()));
            }

            let chunk = self
                .response
                .chunk()
                .await
                .map_err(|error| ClientError::Transport(format!("read events: {error}")))?;

            match chunk {
                Some(bytes) => self.buffer.extend_from_slice(&bytes),
                None => return Ok(None),
            }
        }
    }

    fn take_block(&mut self) -> Option<String> {
        let end = self.buffer.windows(2).position(|pair| pair == b"\n\n")?;
        let rest = self.buffer.split_off(end.saturating_add(2));
        let block = std::mem::replace(&mut self.buffer, rest);

        Some(String::from_utf8_lossy(&block).into_owned())
    }
}

fn parse_block(block: &str) -> Result<Option<SseFrame>, ClientError> {
    let data: Vec<&str> = block
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|line| line.strip_prefix(' ').unwrap_or(line))
        .collect();

    if data.is_empty() {
        return Ok(None);
    }

    serde_json::from_str(&data.join("\n"))
        .map(Some)
        .map_err(|error| ClientError::Protocol(format!("decode SSE frame: {error}")))
}
