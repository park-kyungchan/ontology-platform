use super::{IntakeError, Request, Transport};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Native `gh` credential resolution; GET-only fixed github.com routes. stdout
/// is bounded in-flight, stderr discarded, and each child is killed/reaped on
/// timeout/overflow. No credential appears in arguments, artifacts or errors.
#[derive(Default)]
pub struct GhTransport;
impl Transport for GhTransport {
    fn get(&mut self, request: &Request, max_bytes: usize) -> Result<Vec<u8>, IntakeError> {
        if max_bytes == 0 || max_bytes > 32 * 1024 * 1024 {
            return Err(IntakeError::InvalidConfiguration);
        }
        let mut child = Command::new("gh")
            .args([
                "api",
                "--hostname",
                "github.com",
                "--method",
                "GET",
                &request.route(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| IntakeError::Unavailable)?;
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(IntakeError::Unavailable);
        };
        let (send, receive) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut bytes = vec![];
            let result = stdout.take((max_bytes + 1) as u64).read_to_end(&mut bytes);
            let _ = send.send(result.map(|_| bytes));
        });
        let deadline = Instant::now() + Duration::from_secs(30);
        let received = receive.recv_timeout(Duration::from_secs(30));
        let result = match received {
            Ok(Ok(bytes)) if bytes.len() <= max_bytes => loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        break if status.success() {
                            Ok(bytes)
                        } else {
                            Err(IntakeError::Unavailable)
                        };
                    }
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Ok(None) => break Err(IntakeError::Timeout),
                    Err(_) => break Err(IntakeError::Unavailable),
                }
            },
            Ok(Ok(_)) => Err(IntakeError::LimitExceeded),
            Ok(Err(_)) => Err(IntakeError::Unavailable),
            Err(_) => Err(IntakeError::Timeout),
        };
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
        result
    }
}
