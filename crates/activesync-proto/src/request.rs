/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The request line of an ActiveSync request ([MS-ASHTTP] 2.2.1.1.1): which
//! command, for which device, with which parameters. Clients send it as a
//! plain query (`?Cmd=Sync&User=a&DeviceId=b&DeviceType=c`) or as one
//! base64 value packing the same fields and the protocol version and
//! policy key that would otherwise be headers.

use base64::{Engine, engine::general_purpose::STANDARD};
use std::fmt;

/// The ActiveSync commands, with the codes of the base64 form
/// ([MS-ASHTTP] 2.2.1.1.1.1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    Sync,
    SendMail,
    SmartForward,
    SmartReply,
    GetAttachment,
    FolderSync,
    FolderCreate,
    FolderDelete,
    FolderUpdate,
    MoveItems,
    GetItemEstimate,
    MeetingResponse,
    Search,
    Settings,
    Ping,
    ItemOperations,
    Provision,
    ResolveRecipients,
    ValidateCert,
    Find,
}

impl Command {
    /// Every command with its name and base64 code.
    const ALL: [(Command, &'static str, u8); 20] = [
        (Command::Sync, "Sync", 0),
        (Command::SendMail, "SendMail", 1),
        (Command::SmartForward, "SmartForward", 2),
        (Command::SmartReply, "SmartReply", 3),
        (Command::GetAttachment, "GetAttachment", 4),
        (Command::FolderSync, "FolderSync", 9),
        (Command::FolderCreate, "FolderCreate", 10),
        (Command::FolderDelete, "FolderDelete", 11),
        (Command::FolderUpdate, "FolderUpdate", 12),
        (Command::MoveItems, "MoveItems", 13),
        (Command::GetItemEstimate, "GetItemEstimate", 14),
        (Command::MeetingResponse, "MeetingResponse", 15),
        (Command::Search, "Search", 16),
        (Command::Settings, "Settings", 17),
        (Command::Ping, "Ping", 18),
        (Command::ItemOperations, "ItemOperations", 19),
        (Command::Provision, "Provision", 20),
        (Command::ResolveRecipients, "ResolveRecipients", 21),
        (Command::ValidateCert, "ValidateCert", 22),
        (Command::Find, "Find", 23),
    ];

    pub fn name(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(command, _, _)| *command == self)
            .map_or("", |(_, name, _)| name)
    }

    /// The command named `name` (as in `Cmd=`), ignoring case.
    pub fn from_name(name: &str) -> Option<Command> {
        Self::ALL
            .iter()
            .find(|(_, n, _)| n.eq_ignore_ascii_case(name))
            .map(|(command, _, _)| *command)
    }

    fn from_code(code: u8) -> Option<Command> {
        Self::ALL
            .iter()
            .find(|(_, _, c)| *c == code)
            .map(|(command, _, _)| *command)
    }
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The protocol versions served ([MS-ASHTTP] 2.2.1.1.2.6); a client picks
/// the highest it knows from the `MS-ASProtocolVersions` list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Version {
    V14_1,
    V16_1,
}

impl Version {
    /// The list sent in `MS-ASProtocolVersions`.
    pub const OFFERED: &'static str = "14.1,16.1";

    pub fn parse(value: &str) -> Option<Version> {
        match value.trim() {
            "14.1" => Some(Version::V14_1),
            "16.1" => Some(Version::V16_1),
            _ => None,
        }
    }

    fn from_code(code: u8) -> Option<Version> {
        match code {
            141 => Some(Version::V14_1),
            161 => Some(Version::V16_1),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Version::V14_1 => "14.1",
            Version::V16_1 => "16.1",
        }
    }

    /// Whether the 16.x rules apply (calendar instances, drafts, Find).
    pub fn is_16(self) -> bool {
        self >= Version::V16_1
    }
}

/// Everything the request line says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestLine {
    pub command: Command,
    /// The user name as the device sent it, for logs; the account is the
    /// one that signed in.
    pub user: Option<String>,
    pub device_id: String,
    pub device_type: String,
    /// From the base64 form; the plain form leaves it to the
    /// `MS-ASProtocolVersion` header.
    pub version: Option<Version>,
    /// From the base64 form; the plain form leaves it to `X-MS-PolicyKey`.
    pub policy_key: Option<u32>,
    pub collection_id: Option<String>,
    pub item_id: Option<String>,
    pub long_id: Option<String>,
    pub occurrence: Option<String>,
    pub attachment_name: Option<String>,
    /// SaveInSent (`T`, or the base64 form's option bit 0x01).
    pub save_in_sent: bool,
    /// AcceptMultiPart (the base64 form's option bit 0x02; the header
    /// `MS-ASAcceptMultiPart: T` says the same).
    pub accept_multipart: bool,
}

/// Why the request line was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestLineError {
    /// A required field is missing (`Cmd`, `DeviceId`, `DeviceType`).
    Missing(&'static str),
    /// The command isn't one of ActiveSync's.
    UnknownCommand(String),
    /// The protocol version isn't one this server offers.
    Version(String),
    /// The base64 form is malformed.
    Malformed(&'static str),
    /// A device id with characters [MS-ASHTTP] 2.2.1.1.1.2.3 forbids, or
    /// longer than 32 (the plain form's limit, kept for both forms so the
    /// id can be part of a storage key).
    DeviceId,
}

impl fmt::Display for RequestLineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RequestLineError::Missing(what) => write!(f, "missing {what}"),
            RequestLineError::UnknownCommand(name) => write!(f, "unknown command {name:?}"),
            RequestLineError::Version(v) => write!(f, "protocol version {v:?} isn't offered"),
            RequestLineError::Malformed(what) => write!(f, "malformed base64 query: {what}"),
            RequestLineError::DeviceId => write!(f, "device id must be 1 to 32 letters or digits"),
        }
    }
}

impl std::error::Error for RequestLineError {}

impl RequestLine {
    /// Reads the request line from the URL's query, in either form.
    pub fn parse(query: &str) -> Result<RequestLine, RequestLineError> {
        // Base64 never holds '&', and a plain query always has several
        // parameters.
        if query.contains('&')
            || query
                .get(..4)
                .is_some_and(|s| s.eq_ignore_ascii_case("cmd="))
        {
            Self::parse_plain(query)
        } else {
            Self::parse_base64(query)
        }
    }

    fn empty(command: Command) -> RequestLine {
        RequestLine {
            command,
            user: None,
            device_id: String::new(),
            device_type: String::new(),
            version: None,
            policy_key: None,
            collection_id: None,
            item_id: None,
            long_id: None,
            occurrence: None,
            attachment_name: None,
            save_in_sent: false,
            accept_multipart: false,
        }
    }

    fn parse_plain(query: &str) -> Result<RequestLine, RequestLineError> {
        let mut command = None;
        let mut line = Self::empty(Command::Sync);
        for (key, value) in form_pairs(query) {
            match key.to_ascii_lowercase().as_str() {
                "cmd" => {
                    command = Some(
                        Command::from_name(&value)
                            .ok_or(RequestLineError::UnknownCommand(value))?,
                    )
                }
                "user" => line.user = Some(value),
                "deviceid" => line.device_id = value,
                "devicetype" => line.device_type = value,
                "collectionid" => line.collection_id = Some(value),
                "itemid" => line.item_id = Some(value),
                "longid" => line.long_id = Some(value),
                "occurrence" => line.occurrence = Some(value),
                "attachmentname" => line.attachment_name = Some(value),
                "saveinsent" => line.save_in_sent = value.eq_ignore_ascii_case("T"),
                // Unknown parameters are ignored, as Exchange does.
                _ => (),
            }
        }
        line.command = command.ok_or(RequestLineError::Missing("Cmd"))?;
        line.check()
    }

    fn parse_base64(query: &str) -> Result<RequestLine, RequestLineError> {
        let decoded = percent_decode(query);
        let bytes = STANDARD
            .decode(decoded.trim())
            .map_err(|_| RequestLineError::Malformed("not base64"))?;
        let mut cursor = Cursor {
            bytes: &bytes,
            pos: 0,
        };
        let version_code = cursor.take(1)?[0];
        let version = Version::from_code(version_code)
            .ok_or_else(|| RequestLineError::Version(format!("{version_code}")))?;
        let command_code = cursor.take(1)?[0];
        let command = Command::from_code(command_code)
            .ok_or_else(|| RequestLineError::UnknownCommand(format!("code {command_code}")))?;
        let _locale = cursor.take(2)?;
        let mut line = Self::empty(command);
        line.version = Some(version);
        let len = cursor.take(1)?[0] as usize;
        line.device_id = text(cursor.take(len)?)?;
        match cursor.take(1)?[0] {
            0 => (),
            4 => {
                let key = cursor.take(4)?;
                // Little-endian, as Exchange and every client write it.
                line.policy_key = Some(u32::from_le_bytes([key[0], key[1], key[2], key[3]]));
            }
            _ => return Err(RequestLineError::Malformed("policy key length")),
        }
        let len = cursor.take(1)?[0] as usize;
        line.device_type = text(cursor.take(len)?)?;
        while !cursor.is_done() {
            let tag = cursor.take(1)?[0];
            let len = cursor.take(1)?[0] as usize;
            let value = cursor.take(len)?;
            match tag {
                0 => line.attachment_name = Some(text(value)?),
                1 => line.collection_id = Some(text(value)?),
                3 => line.item_id = Some(text(value)?),
                4 => line.long_id = Some(text(value)?),
                6 => line.occurrence = Some(text(value)?),
                7 => {
                    let options = value.first().copied().unwrap_or(0);
                    line.save_in_sent = options & 0x01 != 0;
                    line.accept_multipart = options & 0x02 != 0;
                }
                8 => line.user = Some(text(value)?),
                _ => (),
            }
        }
        line.check()
    }

    fn check(self) -> Result<RequestLine, RequestLineError> {
        if self.device_id.is_empty() {
            return Err(RequestLineError::Missing("DeviceId"));
        }
        if self.device_type.is_empty() {
            return Err(RequestLineError::Missing("DeviceType"));
        }
        if self.device_id.len() > 32 || !self.device_id.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(RequestLineError::DeviceId);
        }
        Ok(self)
    }
}

/// Reads the base64 form's fields in order.
struct Cursor<'x> {
    bytes: &'x [u8],
    pos: usize,
}

impl<'x> Cursor<'x> {
    fn take(&mut self, len: usize) -> Result<&'x [u8], RequestLineError> {
        let slice = self
            .bytes
            .get(self.pos..self.pos + len)
            .ok_or(RequestLineError::Malformed("too short"))?;
        self.pos += len;
        Ok(slice)
    }

    fn is_done(&self) -> bool {
        self.pos >= self.bytes.len()
    }
}

fn text(bytes: &[u8]) -> Result<String, RequestLineError> {
    String::from_utf8(bytes.to_vec()).map_err(|_| RequestLineError::Malformed("text isn't UTF-8"))
}

/// The `key=value` pairs of a query, percent-decoded (`+` is a space).
fn form_pairs(query: &str) -> impl Iterator<Item = (String, String)> + '_ {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (
                percent_decode(key),
                percent_decode(&value.replace('+', " ")),
            )
        })
}

/// Percent-decoding, keeping malformed escapes as they are.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = bytes.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(std::str::from_utf8(hex).unwrap_or("x"), 16)
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example of [MS-ASHTTP] 2.2.1.1.1.1 (v20250520):
    /// `jAAJBAp2MTQwRGV2aWNlAApTbWFydFBob25l` is version 140, Sync, locale
    /// 0x0409, device "v140Device", no policy key, type "SmartPhone".
    /// 14.0 isn't offered, so it is refused for its version; the same bytes
    /// with 141 are read field by field.
    #[test]
    fn spec_base64_example() {
        assert_eq!(
            RequestLine::parse("jAAJBAp2MTQwRGV2aWNlAApTbWFydFBob25l"),
            Err(RequestLineError::Version("140".into()))
        );
        let mut bytes = STANDARD
            .decode("jAAJBAp2MTQwRGV2aWNlAApTbWFydFBob25l")
            .expect("base64");
        bytes[0] = 141;
        let line = RequestLine::parse(&STANDARD.encode(&bytes)).expect("parses");
        assert_eq!(line.command, Command::Sync);
        assert_eq!(line.version, Some(Version::V14_1));
        assert_eq!(line.device_id, "v140Device");
        assert_eq!(line.device_type, "SmartPhone");
        assert_eq!(line.policy_key, None);
    }

    #[test]
    fn base64_with_policy_key_and_parameters() {
        let mut bytes = vec![161, 3, 0x09, 0x04, 3];
        bytes.extend_from_slice(b"abc");
        bytes.push(4);
        bytes.extend_from_slice(&1234567u32.to_le_bytes());
        bytes.push(6);
        bytes.extend_from_slice(b"iPhone");
        for (tag, value) in [
            (1u8, &b"m:5"[..]),
            (3, b"m:77"),
            (7, &[0x01]),
            (8, b"alice"),
        ] {
            bytes.push(tag);
            bytes.push(value.len() as u8);
            bytes.extend_from_slice(value);
        }
        let line = RequestLine::parse(&STANDARD.encode(&bytes)).expect("parses");
        assert_eq!(line.command, Command::SmartReply);
        assert_eq!(line.version, Some(Version::V16_1));
        assert_eq!(line.policy_key, Some(1234567));
        assert_eq!(line.collection_id.as_deref(), Some("m:5"));
        assert_eq!(line.item_id.as_deref(), Some("m:77"));
        assert!(line.save_in_sent);
        assert_eq!(line.user.as_deref(), Some("alice"));
        // Cut short anywhere: refused, never a panic.
        // (From byte 20 on, after the device type, a cut can fall between
        // two parameters and still be a whole request line.)
        for len in 0..20 {
            assert!(RequestLine::parse(&STANDARD.encode(&bytes[..len])).is_err());
        }
    }

    #[test]
    fn plain_query() {
        let line = RequestLine::parse(
            "Cmd=SmartForward&User=alice%40example.org&DeviceId=ApplF17XXXX&DeviceType=iPhone\
             &CollectionId=m%3A5&ItemId=m%3A9&SaveInSent=T&Foo=bar",
        )
        .expect("parses");
        assert_eq!(line.command, Command::SmartForward);
        assert_eq!(line.user.as_deref(), Some("alice@example.org"));
        assert_eq!(line.collection_id.as_deref(), Some("m:5"));
        assert!(line.save_in_sent);
        assert_eq!(line.version, None);

        for (query, error) in [
            (
                "User=a&DeviceId=b&DeviceType=c",
                RequestLineError::Missing("Cmd"),
            ),
            (
                "Cmd=Sync&User=a&DeviceType=c",
                RequestLineError::Missing("DeviceId"),
            ),
            (
                "Cmd=Sync&DeviceId=b",
                RequestLineError::Missing("DeviceType"),
            ),
            (
                "Cmd=Nope&DeviceId=b&DeviceType=c",
                RequestLineError::UnknownCommand("Nope".into()),
            ),
            (
                "Cmd=Sync&DeviceId=a,b&DeviceType=c",
                RequestLineError::DeviceId,
            ),
        ] {
            assert_eq!(RequestLine::parse(query), Err(error), "{query}");
        }
    }
}
