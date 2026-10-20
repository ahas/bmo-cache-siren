//! The bearer token every API route takes (docs/api.md, Access): the content of one file, which the daemon
//! writes with a new random token, readable by its user alone, when the file is missing.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

/// How many random bytes a new token holds; it is written as twice as many hex digits.
const TOKEN_BYTES: usize = 32;

/// The token in `path`, its surrounding whitespace dropped; an empty file is an error.
pub fn read_token(path: &Path) -> io::Result<String> {
  let token = fs::read_to_string(path)?.trim().to_string();
  if token.is_empty() {
    return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{} holds no token", path.display())));
  }
  Ok(token)
}

/// The token in `path`, written there first with a new random one where the file is missing.
pub fn read_or_make_token(path: &Path) -> io::Result<String> {
  match read_token(path) {
    Err(why) if why.kind() == io::ErrorKind::NotFound => {}
    read => return read,
  }
  let mut bytes = [0u8; TOKEN_BYTES];
  getrandom::fill(&mut bytes).map_err(io::Error::other)?;
  let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
  if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
    fs::create_dir_all(dir)?;
  }
  let mut options = OpenOptions::new();
  options.write(true).create_new(true);
  #[cfg(unix)]
  std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
  let mut file = options.open(path)?;
  file.write_all(format!("{token}\n").as_bytes())?;
  Ok(token)
}
