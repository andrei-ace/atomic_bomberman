use crate::NetError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::{
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
};
use url::Url;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerProfile {
    pub endpoint: String,
    pub fingerprint: [u8; 32],
    pub token: String,
}
impl fmt::Debug for ServerProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ServerProfile([private])")
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invitation {
    pub server: ServerProfile,
    pub room: Option<String>,
}
impl fmt::Debug for Invitation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Invitation([private])")
    }
}
pub fn random_token() -> String {
    let mut bytes = [0; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
pub fn valid_room(room: &str) -> bool {
    room.len() == 8
        && room
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}
impl Invitation {
    pub fn encode(&self) -> Result<String, NetError> {
        self.validate()?;
        let endpoint = Url::parse(&self.server.endpoint).map_err(|_| NetError::InvalidInvite)?;
        let host = endpoint.host().ok_or(NetError::InvalidInvite)?;
        let pin: String = self
            .server
            .fingerprint
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut fragment = url::form_urlencoded::Serializer::new(String::new());
        fragment
            .append_pair("v", "1")
            .append_pair("token", &self.server.token)
            .append_pair("pin", &pin);
        if let Some(room) = &self.room {
            fragment.append_pair("room", room);
        }
        Ok(format!(
            "atomic-bomberman://{host}:{}/#{}",
            endpoint
                .port_or_known_default()
                .ok_or(NetError::InvalidInvite)?,
            fragment.finish()
        ))
    }
    pub fn parse(text: &str, saved: Option<&ServerProfile>) -> Result<Self, NetError> {
        let text = text.trim();
        let result = if text.starts_with("atomic-bomberman:") {
            let url = Url::parse(text).map_err(|_| NetError::InvalidInvite)?;
            if text.len() > 4096
                || url.scheme() != "atomic-bomberman"
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.path() != "/"
            {
                return Err(NetError::InvalidInvite);
            }
            let mut version = None;
            let mut token = None;
            let mut pin = None;
            let mut room = None;
            for (key, value) in url::form_urlencoded::parse(
                url.fragment().ok_or(NetError::InvalidInvite)?.as_bytes(),
            ) {
                let field = match key.as_ref() {
                    "v" => &mut version,
                    "token" => &mut token,
                    "pin" => &mut pin,
                    "room" => &mut room,
                    _ => return Err(NetError::InvalidInvite),
                };
                if field.replace(value.into_owned()).is_some() {
                    return Err(NetError::InvalidInvite);
                }
            }
            if version.as_deref() != Some("1") {
                return Err(NetError::InvalidInvite);
            }
            let pin = pin.ok_or(NetError::InvalidInvite)?;
            if pin.len() != 64 || !pin.is_ascii() {
                return Err(NetError::InvalidInvite);
            }
            let mut fingerprint = [0; 32];
            for (i, byte) in fingerprint.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&pin[i * 2..i * 2 + 2], 16)
                    .map_err(|_| NetError::InvalidInvite)?;
            }
            Self {
                server: ServerProfile {
                    endpoint: format!(
                        "wss://{}:{}/v1/ws",
                        url.host().ok_or(NetError::InvalidInvite)?,
                        url.port().unwrap_or(443)
                    ),
                    fingerprint,
                    token: token.ok_or(NetError::InvalidInvite)?,
                },
                room,
            }
        } else if let Some(encoded) = text.strip_prefix("AB1.") {
            if encoded.len() > 4096 {
                return Err(NetError::InvalidInvite);
            }
            let bytes = URL_SAFE_NO_PAD
                .decode(encoded)
                .map_err(|_| NetError::InvalidInvite)?;
            serde_json::from_slice(&bytes).map_err(|_| NetError::InvalidInvite)?
        } else {
            let room = text.to_ascii_uppercase();
            if !valid_room(&room) {
                return Err(NetError::InvalidInvite);
            }
            Self {
                server: saved.ok_or(NetError::InvalidInvite)?.clone(),
                room: Some(room),
            }
        };
        result.validate()?;
        Ok(result)
    }
    pub fn validate(&self) -> Result<(), NetError> {
        let url = Url::parse(&self.server.endpoint).map_err(|_| NetError::InvalidInvite)?;
        if url.scheme() != "wss"
            || url.host_str().is_none()
            || url.port_or_known_default().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/v1/ws"
            || self.server.endpoint.len() > 512
            || URL_SAFE_NO_PAD
                .decode(&self.server.token)
                .map_or(true, |v| v.len() != 32)
            || self.room.as_ref().is_some_and(|r| !valid_room(r))
        {
            return Err(NetError::InvalidInvite);
        }
        Ok(())
    }
}
pub fn profile_path() -> PathBuf {
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(base).join("atomic-bomberman/connections.json");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Atomic Bomberman/connections.json")
    } else {
        home.join(".config/atomic-bomberman/connections.json")
    }
}
impl ServerProfile {
    pub fn load(path: &Path) -> Result<Option<Self>, NetError> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(NetError::PrivateFile),
        };
        if !metadata.is_file() || metadata.len() > 4096 {
            return Err(NetError::PrivateFile);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(NetError::PrivateFile);
            }
        }
        let server: Self =
            serde_json::from_slice(&fs::read(path).map_err(|_| NetError::PrivateFile)?)
                .map_err(|_| NetError::PrivateFile)?;
        Invitation {
            server: server.clone(),
            room: None,
        }
        .validate()
        .map_err(|_| NetError::PrivateFile)?;
        Ok(Some(server))
    }
    pub fn save(&self, path: &Path) -> Result<(), NetError> {
        Invitation {
            server: self.clone(),
            room: None,
        }
        .validate()?;
        let parent = path.parent().ok_or(NetError::PrivateFile)?;
        fs::create_dir_all(parent).map_err(|_| NetError::PrivateFile)?;
        if fs::symlink_metadata(parent)
            .map_err(|_| NetError::PrivateFile)?
            .file_type()
            .is_symlink()
            || fs::symlink_metadata(path).is_ok_and(|m| !m.is_file() || m.file_type().is_symlink())
        {
            return Err(NetError::PrivateFile);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                .map_err(|_| NetError::PrivateFile)?;
        }
        let temporary = parent.join(format!(".connections-{}", random_token()));
        let result = (|| -> std::io::Result<()> {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&serde_json::to_vec(self)?)?;
            file.sync_all()?;
            fs::rename(&temporary, path)?;
            fs::File::open(parent)?.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(|_| NetError::PrivateFile)
    }
}
