/*
 * SPDX-FileCopyrightText: 2026 Hearth Mail
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Autodiscover in its SOAP dialect ([MS-OXWSADISC]):
//! `POST /autodiscover/autodiscover.svc` with `GetUserSettings` or
//! `GetDomainSettings`.
//!
//! What Hearth Mail can say is answered: the address, a display name (the
//! address: the request is anonymous, so no name is given away), and the
//! IMAP, POP3 and SMTP connection settings, the same the POX and
//! Thunderbird answers give. Everything else a client asks for (EWS, ECP,
//! the offline address book) is answered `SettingIsNotAvailable`: Hearth
//! Mail has none of them. The SOAP dialect has no ActiveSync setting
//! (MS-OXWSADISC 3.1.4.4), so ActiveSync clients learn the address from
//! the POX or JSON dialect. Only domains this server serves get settings.

use crate::{Server, manager::application::Resource};
use quick_xml::{NsReader, XmlVersion, escape::escape, events::Event, name::ResolveResult};
use registry::schema::{enums::ServiceProtocol, structs::Service};
use std::fmt::Write;
use utils::map::vec_map::VecMap;

const SOAP_11: &str = "http://schemas.xmlsoap.org/soap/envelope/";
const SOAP_12: &str = "http://www.w3.org/2003/05/soap-envelope";
const AUTODISCOVER_NS: &str = "http://schemas.microsoft.com/exchange/2010/Autodiscover";
/// The same placeholder the POX answer gives: Hearth Mail has no Exchange
/// deployment, but the field is required.
const DEPLOYMENT_ID: &str = "644560b8-a1ce-429c-8ace-23395843f701";
/// At most this many users or domains are answered per request.
const MAX_ENTRIES: usize = 100;

/// What a SOAP request asks.
#[derive(Debug, Default, PartialEq, Eq)]
struct SoapRequest {
    /// The envelope's namespace, answered in kind (SOAP 1.1 or 1.2).
    soap_12: bool,
    operation: Operation,
    /// Mailboxes (GetUserSettings) or domains (GetDomainSettings).
    entries: Vec<String>,
    settings: Vec<String>,
}

#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
enum Operation {
    #[default]
    Unknown,
    GetUserSettings,
    GetDomainSettings,
}

impl Server {
    /// Answers a SOAP Autodiscover request. The answer is always SOAP: a
    /// fault for a body that can't be read, otherwise per-user errors.
    pub async fn handle_autodiscover_soap_request(
        &self,
        body: Option<Vec<u8>>,
    ) -> trc::Result<(u16, Resource<Vec<u8>>)> {
        let request = match parse_soap_request(body.as_deref().unwrap_or_default()) {
            Ok(request) => request,
            Err(reason) => return Ok((500, fault(false, "s:Client", &reason))),
        };
        let soap_12 = request.soap_12;
        let body = match request.operation {
            Operation::GetUserSettings => {
                let mut users = Vec::with_capacity(request.entries.len());
                let mut off = Vec::with_capacity(request.entries.len());
                for mailbox in &request.entries {
                    let domain = mailbox.rsplit_once('@').map_or("", |(_, domain)| domain);
                    let local = !domain.is_empty() && self.domain(domain).await?.is_some();
                    users.push((mailbox.as_str(), local));
                    // inbuxa: legacy-protocols LP-7, LP-14a, as the POX answer
                    off.push((mailbox.as_str(), self.legacy_off_for(domain).await?));
                }
                user_settings_response(
                    &users,
                    &request.settings,
                    &self.core.network.server_name,
                    &self.core.network.info.services,
                    |mailbox, protocol| {
                        off.iter()
                            .find(|(m, _)| *m == mailbox)
                            .is_some_and(|(_, legacy_off)| legacy_off.service(protocol))
                    },
                )
            }
            Operation::GetDomainSettings => {
                let mut domains = Vec::with_capacity(request.entries.len());
                for domain in &request.entries {
                    domains.push((domain.as_str(), self.domain(domain).await?.is_some()));
                }
                domain_settings_response(&domains, &request.settings)
            }
            Operation::Unknown => {
                return Ok((
                    500,
                    fault(
                        soap_12,
                        "s:Client",
                        "Only GetUserSettings and GetDomainSettings are answered",
                    ),
                ));
            }
        };
        Ok((200, envelope(soap_12, request.operation, &body)))
    }
}

/// Reads the operation, the users or domains and the settings asked for.
/// Element names are matched by local name within the Autodiscover
/// namespace, so any prefixes work.
fn parse_soap_request(bytes: &[u8]) -> Result<SoapRequest, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "the request isn't UTF-8".to_string())?;
    let mut reader = NsReader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut request = SoapRequest::default();
    let mut saw_envelope = false;
    // The local name of the element whose text is wanted next.
    let mut current: Option<Vec<u8>> = None;
    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|e| format!("XML: {e}"))?;
        match event {
            Event::Start(start) => {
                let ns = match &namespace {
                    ResolveResult::Bound(ns) => ns.as_ref(),
                    _ => b"",
                };
                let local = start.local_name().as_ref().to_vec();
                if local == b"Envelope" {
                    saw_envelope = true;
                    request.soap_12 = ns == SOAP_12.as_bytes();
                    if !request.soap_12 && ns != SOAP_11.as_bytes() {
                        return Err("not a SOAP envelope".into());
                    }
                } else if ns == AUTODISCOVER_NS.as_bytes() {
                    match local.as_slice() {
                        b"GetUserSettingsRequestMessage" => {
                            request.operation = Operation::GetUserSettings
                        }
                        b"GetDomainSettingsRequestMessage" => {
                            request.operation = Operation::GetDomainSettings
                        }
                        b"Mailbox" | b"Domain" | b"Setting" => current = Some(local),
                        _ => (),
                    }
                }
            }
            Event::Text(text) => {
                if let Some(local) = current.take() {
                    let value = text
                        .xml_content(XmlVersion::Implicit1_0)
                        .map_err(|e| format!("XML text: {e}"))?
                        .trim()
                        .to_string();
                    let list = if local == b"Setting" {
                        &mut request.settings
                    } else {
                        &mut request.entries
                    };
                    if list.len() >= MAX_ENTRIES {
                        return Err(format!("more than {MAX_ENTRIES} entries"));
                    }
                    list.push(if local == b"Setting" {
                        value
                    } else {
                        value.to_lowercase()
                    });
                }
            }
            Event::End(_) => current = None,
            Event::Eof => break,
            _ => (),
        }
    }
    if !saw_envelope {
        return Err("not a SOAP envelope".into());
    }
    Ok(request)
}

/// The connection settings of one protocol, for the requested setting
/// names that ask for them (internal and external are the same here).
fn protocol_of(setting: &str) -> Option<ServiceProtocol> {
    match setting {
        "ExternalImap4Connections" | "InternalImap4Connections" => Some(ServiceProtocol::Imap),
        "ExternalPop3Connections" | "InternalPop3Connections" => Some(ServiceProtocol::Pop3),
        "ExternalSmtpConnections" | "InternalSmtpConnections" => Some(ServiceProtocol::Smtp),
        _ => None,
    }
}

/// `users` are the mailboxes asked about and whether this server serves
/// their domain; `switched_off` says whether a protocol is off for a
/// mailbox (the legacy-protocol policy, as in the POX answer).
fn user_settings_response(
    users: &[(&str, bool)],
    settings: &[String],
    server_name: &str,
    services: &VecMap<ServiceProtocol, Service>,
    switched_off: impl Fn(&str, &ServiceProtocol) -> bool,
) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("<UserResponses>");
    for (mailbox_raw, local) in users {
        let mailbox_raw = *mailbox_raw;
        let mailbox = escape(mailbox_raw);
        out.push_str("<UserResponse>");
        if !local {
            out.push_str(
                "<ErrorCode>InvalidUser</ErrorCode>\
                 <ErrorMessage>This address isn't served here.</ErrorMessage>\
                 <RedirectTarget i:nil=\"true\" /><UserSettingErrors /><UserSettings />\
                 </UserResponse>",
            );
            continue;
        }
        let mut values = String::new();
        let mut errors = String::new();
        for setting in settings {
            let name = escape(setting.as_str());
            let value = match setting.as_str() {
                "UserDisplayName" | "AutoDiscoverSMTPAddress" => Some(mailbox.to_string()),
                "UserDeploymentId" => Some(DEPLOYMENT_ID.to_string()),
                _ => None,
            };
            if let Some(value) = value {
                let _ = write!(
                    values,
                    "<UserSetting i:type=\"StringSetting\"><Name>{name}</Name>\
                     <Value>{value}</Value></UserSetting>"
                );
            } else if let Some((protocol, service)) = protocol_of(setting)
                .filter(|protocol| !switched_off(mailbox_raw, protocol))
                .and_then(|protocol| services.get(&protocol).map(|service| (protocol, service)))
            {
                let _ = write!(
                    values,
                    "<UserSetting i:type=\"ProtocolConnectionCollectionSetting\">\
                     <Name>{name}</Name><ProtocolConnections>"
                );
                for (port, encryption) in connections(protocol, service) {
                    let host = escape(service.hostname.as_deref().unwrap_or(server_name));
                    let _ = write!(
                        values,
                        "<ProtocolConnection><Hostname>{host}</Hostname>\
                         <Port>{port}</Port><EncryptionMethod>{encryption}</EncryptionMethod>\
                         </ProtocolConnection>"
                    );
                }
                values.push_str("</ProtocolConnections></UserSetting>");
            } else {
                let _ = write!(
                    errors,
                    "<UserSettingError><ErrorCode>SettingIsNotAvailable</ErrorCode>\
                     <ErrorMessage>Not offered by this server.</ErrorMessage>\
                     <SettingName>{name}</SettingName></UserSettingError>"
                );
            }
        }
        let _ = write!(
            out,
            "<ErrorCode>NoError</ErrorCode><ErrorMessage>No error.</ErrorMessage>\
             <RedirectTarget i:nil=\"true\" />\
             <UserSettingErrors>{errors}</UserSettingErrors>\
             <UserSettings>{values}</UserSettings></UserResponse>"
        );
    }
    out.push_str("</UserResponses>");
    out
}

/// Implicit TLS first (RFC 8314), then STARTTLS when cleartext ports are
/// open, as the POX answer lists them.
fn connections(protocol: ServiceProtocol, service: &Service) -> Vec<(u16, &'static str)> {
    let (tls, starttls) = match protocol {
        ServiceProtocol::Imap => (993, 143),
        ServiceProtocol::Pop3 => (995, 110),
        _ => (465, 587),
    };
    let mut list = vec![(tls, "SSL")];
    if service.cleartext {
        list.push((starttls, "TLS"));
    }
    list
}

fn domain_settings_response(domains: &[(&str, bool)], settings: &[String]) -> String {
    let mut out = String::from("<DomainResponses>");
    for (_, local) in domains {
        if !local {
            out.push_str(
                "<DomainResponse><ErrorCode>InvalidDomain</ErrorCode>\
                 <ErrorMessage>This domain isn't served here.</ErrorMessage>\
                 <DomainSettingErrors /><DomainSettings /><RedirectTarget i:nil=\"true\" />\
                 </DomainResponse>",
            );
            continue;
        }
        out.push_str(
            "<DomainResponse><ErrorCode>NoError</ErrorCode><ErrorMessage>No error.</ErrorMessage>\
             <DomainSettingErrors>",
        );
        for setting in settings {
            let _ = write!(
                out,
                "<DomainSettingError><ErrorCode>SettingIsNotAvailable</ErrorCode>\
                 <ErrorMessage>Not offered by this server.</ErrorMessage>\
                 <SettingName>{}</SettingName></DomainSettingError>",
                escape(setting.as_str())
            );
        }
        out.push_str(
            "</DomainSettingErrors><DomainSettings /><RedirectTarget i:nil=\"true\" />\
             </DomainResponse>",
        );
    }
    out.push_str("</DomainResponses>");
    out
}

fn envelope(soap_12: bool, operation: Operation, inner: &str) -> Resource<Vec<u8>> {
    let (soap_ns, content_type) = soap_version(soap_12);
    let name = match operation {
        Operation::GetDomainSettings => "GetDomainSettings",
        _ => "GetUserSettings",
    };
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <s:Envelope xmlns:s=\"{soap_ns}\" xmlns:a=\"http://www.w3.org/2005/08/addressing\">\
         <s:Header><a:Action s:mustUnderstand=\"1\">{AUTODISCOVER_NS}/Autodiscover/{name}Response</a:Action>\
         </s:Header><s:Body>\
         <{name}ResponseMessage xmlns=\"{AUTODISCOVER_NS}\">\
         <Response xmlns:i=\"http://www.w3.org/2001/XMLSchema-instance\">\
         <ErrorCode>NoError</ErrorCode><ErrorMessage />{inner}</Response>\
         </{name}ResponseMessage></s:Body></s:Envelope>"
    );
    Resource::new(content_type, body.into_bytes())
}

fn fault(soap_12: bool, code: &str, reason: &str) -> Resource<Vec<u8>> {
    let (soap_ns, content_type) = soap_version(soap_12);
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <s:Envelope xmlns:s=\"{soap_ns}\"><s:Body><s:Fault>\
         <faultcode>{code}</faultcode><faultstring>{}</faultstring>\
         </s:Fault></s:Body></s:Envelope>",
        escape(reason)
    );
    Resource::new(content_type, body.into_bytes())
}

fn soap_version(soap_12: bool) -> (&'static str, &'static str) {
    if soap_12 {
        (SOAP_12, "application/soap+xml; charset=utf-8")
    } else {
        (SOAP_11, "text/xml; charset=utf-8")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The request of [MS-OXWSADISC] 4.3, without the PDF's line breaks.
    const SPEC_REQUEST: &str = r#"<?xml version="1.0" encoding="utf-8"?>
        <soap:Envelope xmlns:a="http://schemas.microsoft.com/exchange/2010/Autodiscover" xmlns:wsa="http://www.w3.org/2005/08/addressing" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
          <soap:Header>
            <a:RequestedServerVersion>Exchange2010</a:RequestedServerVersion>
            <wsa:Action>http://schemas.microsoft.com/exchange/2010/Autodiscover/Autodiscover/GetUserSettings</wsa:Action>
            <wsa:To>https://autodiscover.exchange.microsoft.com/autodiscover/autodiscover.svc</wsa:To>
          </soap:Header>
          <soap:Body>
            <a:GetUserSettingsRequestMessage xmlns:a="http://schemas.microsoft.com/exchange/2010/Autodiscover">
              <a:Request>
                <a:Users>
                  <a:User>
                    <a:Mailbox>tadam@exchange.microsoft.com</a:Mailbox>
                  </a:User>
                </a:Users>
                <a:RequestedSettings>
                  <a:Setting>UserDisplayName</a:Setting>
                  <a:Setting>UserDeploymentId</a:Setting>
                  <a:Setting>ExternalEwsUrl</a:Setting>
                  <a:Setting>ExternalImap4Connections</a:Setting>
                </a:RequestedSettings>
              </a:Request>
            </a:GetUserSettingsRequestMessage>
          </soap:Body>
        </soap:Envelope>"#;

    #[test]
    fn reads_the_spec_request() {
        let request = parse_soap_request(SPEC_REQUEST.as_bytes()).expect("parses");
        assert_eq!(request.operation, Operation::GetUserSettings);
        assert!(!request.soap_12);
        assert_eq!(request.entries, ["tadam@exchange.microsoft.com"]);
        assert_eq!(
            request.settings,
            [
                "UserDisplayName",
                "UserDeploymentId",
                "ExternalEwsUrl",
                "ExternalImap4Connections"
            ]
        );
    }

    #[test]
    fn answers_what_it_has() {
        let request = parse_soap_request(SPEC_REQUEST.as_bytes()).expect("parses");
        let services: VecMap<ServiceProtocol, Service> = [(
            ServiceProtocol::Imap,
            Service {
                hostname: None,
                cleartext: false,
            },
        )]
        .into_iter()
        .collect();
        let xml = user_settings_response(
            &[
                ("tadam@exchange.microsoft.com", true),
                ("x@else.test", false),
            ],
            &request.settings,
            "mail.example.org",
            &services,
            |_, _| false,
        );
        assert!(
            xml.contains("<Name>UserDisplayName</Name><Value>tadam@exchange.microsoft.com</Value>"),
            "{xml}"
        );
        assert!(
            xml.contains("<Hostname>mail.example.org</Hostname><Port>993</Port><EncryptionMethod>SSL</EncryptionMethod>"),
            "{xml}"
        );
        assert!(!xml.contains("<Port>143</Port>"), "{xml}");
        assert!(
            xml.contains("<ErrorCode>SettingIsNotAvailable</ErrorCode><ErrorMessage>Not offered by this server.</ErrorMessage><SettingName>ExternalEwsUrl</SettingName>"),
            "{xml}"
        );
        assert!(xml.contains("<ErrorCode>InvalidUser</ErrorCode>"), "{xml}");
    }

    #[test]
    fn refuses_what_is_not_soap() {
        assert!(parse_soap_request(b"<Autodiscover/>").is_err());
        assert!(parse_soap_request(b"not xml <").is_err());
        let soap_12 = SPEC_REQUEST.replace(SOAP_11, SOAP_12);
        assert!(
            parse_soap_request(soap_12.as_bytes())
                .expect("parses")
                .soap_12
        );
        let many = format!(
            r#"<s:Envelope xmlns:s="{SOAP_11}"><s:Body><GetUserSettingsRequestMessage xmlns="{AUTODISCOVER_NS}"><Request><Users>{}</Users></Request></GetUserSettingsRequestMessage></s:Body></s:Envelope>"#,
            "<User><Mailbox>a@b.c</Mailbox></User>".repeat(MAX_ENTRIES + 1)
        );
        assert!(parse_soap_request(many.as_bytes()).is_err());
    }
}
