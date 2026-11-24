/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 *
 * Modified by Coffey Labs in 2026 for INBUXA.
 * Modified by Hearth Mail in 2026: the ActiveSync (mobilesync) schema.
 */

//! Autodiscover in its POX dialect: `POST /autodiscover/autodiscover.xml`
//! with the address and the response schema the client accepts. Outlook's
//! schema lists the IMAP, POP3 and SMTP settings; ActiveSync's (mobilesync,
//! [MS-ASCMD] 2.2.1.1) gives the ActiveSync address, for domains this
//! server serves.

use crate::{Server, manager::application::Resource};
use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::escape::escape;
use quick_xml::events::Event;
use registry::schema::{enums::ServiceProtocol, structs::Service};
use std::{borrow::Cow, fmt::Write};
use utils::map::vec_map::VecMap;

impl Server {
    pub async fn handle_autodiscover_request(
        &self,
        body: Option<Vec<u8>>,
    ) -> trc::Result<Resource<Vec<u8>>> {
        // Obtain parameters
        let request =
            parse_autodiscover_request(body.as_deref().unwrap_or_default()).map_err(|err| {
                trc::ResourceEvent::BadParameters
                    .into_err()
                    .details("Failed to parse autodiscover request")
                    .ctx(trc::Key::Reason, err)
            })?;
        // inbuxa: legacy-protocols LP-7, LP-14a
        let legacy_off = match request.email.rsplit_once('@') {
            Some((_, domain)) => self.legacy_off_for(domain).await?,
            None => self.legacy_off_for("").await?,
        };

        let response = match request.response_schema {
            // Hearth Mail: ActiveSync devices (docs/activesync.md)
            ResponseSchema::MobileSync => match request.email.rsplit_once('@') {
                Some((_, domain)) if self.domain(domain).await?.is_some() => {
                    build_mobilesync_response(&request.email, &self.core.network.server_name)
                }
                _ => build_mobilesync_error(&request.email),
            }
            .into_bytes(),
            ResponseSchema::Outlook => build_autodiscover_response(
                &request.email,
                &self.core.network.server_name,
                &self.core.network.info.services,
                |protocol| legacy_off.service(protocol),
            )
            .into_bytes(),
            ResponseSchema::Unsupported => PROVIDER_NOT_AVAILABLE_RESPONSE.as_bytes().to_vec(),
        };

        Ok(Resource::new("application/xml; charset=utf-8", response))
    }
}

const OUTLOOK_RESPONSE_SCHEMA: &str =
    "http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a";

const MOBILESYNC_RESPONSE_SCHEMA: &str =
    "http://schemas.microsoft.com/exchange/autodiscover/mobilesync/responseschema/2006";

/// Where ActiveSync is served, under the server's own name.
pub fn activesync_url(server_name: &str) -> String {
    format!("https://{server_name}/Microsoft-Server-ActiveSync")
}

/// The mobilesync answer with the ActiveSync address, as in [MS-ASCMD]
/// 4.2.4. `Url` and `Name` carry the same address, as Exchange sends them.
fn build_mobilesync_response(email: &str, server_name: &str) -> String {
    let email = escape(email);
    let url = escape(activesync_url(server_name));
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <Autodiscover xmlns=\"http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006\">\n\
         \t<Response xmlns=\"{MOBILESYNC_RESPONSE_SCHEMA}\">\n\
         \t\t<Culture>en:us</Culture>\n\
         \t\t<User>\n\t\t\t<DisplayName>{email}</DisplayName>\n\
         \t\t\t<EMailAddress>{email}</EMailAddress>\n\t\t</User>\n\
         \t\t<Action>\n\t\t\t<Settings>\n\t\t\t\t<Server>\n\
         \t\t\t\t\t<Type>MobileSync</Type>\n\
         \t\t\t\t\t<Url>{url}</Url>\n\
         \t\t\t\t\t<Name>{url}</Name>\n\
         \t\t\t\t</Server>\n\t\t\t</Settings>\n\t\t</Action>\n\
         \t</Response>\n\
         </Autodiscover>\n"
    )
}

/// The mobilesync error for an address this server doesn't serve, shaped as
/// [MS-ASCMD] 4.2.2 (status 2, a protocol error), so the device asks the
/// person for the settings instead.
fn build_mobilesync_error(email: &str) -> String {
    let email = escape(email);
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <Autodiscover xmlns=\"http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006\">\n\
         \t<Response xmlns=\"{MOBILESYNC_RESPONSE_SCHEMA}\">\n\
         \t\t<Culture>en:us</Culture>\n\
         \t\t<User>\n\t\t\t<EMailAddress>{email}</EMailAddress>\n\t\t</User>\n\
         \t\t<Action>\n\t\t\t<Error>\n\
         \t\t\t\t<Status>2</Status>\n\
         \t\t\t\t<Message>This address isn't served here</Message>\n\
         \t\t\t\t<DebugData />\n\
         \t\t\t</Error>\n\t\t</Action>\n\
         \t</Response>\n\
         </Autodiscover>\n"
    )
}

const PROVIDER_NOT_AVAILABLE_RESPONSE: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
    "<Autodiscover xmlns=\"http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006\">\n",
    "\t<Response>\n",
    "\t\t<Error>\n",
    "\t\t\t<ErrorCode>601</ErrorCode>\n",
    "\t\t\t<Message>Provider is not available</Message>\n",
    "\t\t\t<DebugData />\n",
    "\t\t</Error>\n",
    "\t</Response>\n",
    "</Autodiscover>\n",
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponseSchema {
    Outlook,
    MobileSync,
    Unsupported,
}

impl ResponseSchema {
    fn parse(value: &str) -> Self {
        // Whitespace inside the value is ignored: the specification's own
        // example wraps it over two lines.
        let value: String = value.split_whitespace().collect();
        if value.eq_ignore_ascii_case(OUTLOOK_RESPONSE_SCHEMA) {
            ResponseSchema::Outlook
        } else if value.eq_ignore_ascii_case(MOBILESYNC_RESPONSE_SCHEMA) {
            ResponseSchema::MobileSync
        } else {
            ResponseSchema::Unsupported
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct AutodiscoverRequest {
    email: String,
    response_schema: ResponseSchema,
}

#[derive(Clone, Copy)]
enum RequestField {
    EmailAddress,
    ResponseSchema,
}

fn build_autodiscover_response(
    emailaddress: &str,
    default_host: &str,
    services: &VecMap<ServiceProtocol, Service>,
    switched_off: impl Fn(&ServiceProtocol) -> bool,
) -> String {
    let emailaddress = escape(emailaddress);

    // Build XML response
    let mut config = String::with_capacity(1024);
    let _ = writeln!(&mut config, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    let _ = writeln!(
        &mut config,
        "<Autodiscover xmlns=\"http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006\">"
    );
    let _ = writeln!(
        &mut config,
        "\t<Response xmlns=\"http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a\">"
    );
    let _ = writeln!(&mut config, "\t\t<User>");
    let _ = writeln!(
        &mut config,
        "\t\t\t<DisplayName>{emailaddress}</DisplayName>"
    );
    let _ = writeln!(
        &mut config,
        "\t\t\t<AutoDiscoverSMTPAddress>{emailaddress}</AutoDiscoverSMTPAddress>"
    );
    // DeploymentId is a required field of User but we are not a MS Exchange server so use a random value
    let _ = writeln!(
        &mut config,
        "\t\t\t<DeploymentId>644560b8-a1ce-429c-8ace-23395843f701</DeploymentId>"
    );
    let _ = writeln!(&mut config, "\t\t</User>");
    let _ = writeln!(&mut config, "\t\t<Account>");
    let _ = writeln!(&mut config, "\t\t\t<AccountType>email</AccountType>");
    let _ = writeln!(&mut config, "\t\t\t<Action>settings</Action>");
    for (protocol, service) in services {
        if switched_off(protocol) {
            continue;
        }
        let (protocol, ports) = match protocol {
            ServiceProtocol::Imap => ("IMAP", [(993, true), (143, false)]),
            ServiceProtocol::Pop3 => ("POP3", [(995, true), (110, false)]),
            ServiceProtocol::Smtp => ("SMTP", [(465, true), (587, false)]),
            _ => continue,
        };

        // Implicit TLS is listed first so that it is preferred (RFC 8314)
        for (port, is_tls) in ports {
            if is_tls || service.cleartext {
                let server_name = service.hostname.as_deref().unwrap_or(default_host);
                let _ = writeln!(&mut config, "\t\t\t<Protocol>");
                let _ = writeln!(&mut config, "\t\t\t\t<Type>{protocol}</Type>",);
                let _ = writeln!(&mut config, "\t\t\t\t<Server>{server_name}</Server>");
                let _ = writeln!(&mut config, "\t\t\t\t<Port>{port}</Port>");
                let _ = writeln!(&mut config, "\t\t\t\t<LoginName>{emailaddress}</LoginName>");
                let _ = writeln!(&mut config, "\t\t\t\t<AuthRequired>on</AuthRequired>");
                let _ = writeln!(&mut config, "\t\t\t\t<DirectoryPort>0</DirectoryPort>");
                let _ = writeln!(&mut config, "\t\t\t\t<ReferralPort>0</ReferralPort>");
                let (ssl, encryption) = if is_tls {
                    ("on", "SSL")
                } else {
                    ("off", "TLS")
                };
                let _ = writeln!(&mut config, "\t\t\t\t<SSL>{ssl}</SSL>");
                let _ = writeln!(&mut config, "\t\t\t\t<Encryption>{encryption}</Encryption>");
                let _ = writeln!(&mut config, "\t\t\t\t<SPA>off</SPA>");
                let _ = writeln!(&mut config, "\t\t\t</Protocol>");
            }
        }
    }

    let _ = writeln!(&mut config, "\t\t</Account>");
    let _ = writeln!(&mut config, "\t</Response>");
    let _ = writeln!(&mut config, "</Autodiscover>");

    config
}

fn parse_autodiscover_request(bytes: &[u8]) -> Result<AutodiscoverRequest, String> {
    if bytes.is_empty() {
        return Err("Empty request body".to_string());
    }

    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::with_capacity(128);
    let mut value_buf = Vec::with_capacity(128);

    'outer: for tag_name in ["Autodiscover", "Request"] {
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let found_tag_name = e.name();
                    if tag_name
                        .as_bytes()
                        .eq_ignore_ascii_case(found_tag_name.as_ref())
                    {
                        continue 'outer;
                    } else {
                        return Err(format!(
                            "Expected tag {}, found unexpected tag {} at position {}.",
                            tag_name,
                            String::from_utf8_lossy(found_tag_name.as_ref()),
                            reader.buffer_position()
                        ));
                    }
                }
                Ok(Event::Decl(_) | Event::Text(_)) => (),
                Err(e) => {
                    return Err(format!(
                        "Error at position {}: {:?}",
                        reader.buffer_position(),
                        e
                    ));
                }
                Ok(event) => {
                    return Err(format!(
                        "Expected tag {}, found unexpected event {event:?} at position {}.",
                        tag_name,
                        reader.buffer_position()
                    ));
                }
            }
        }
    }

    let mut email = None;
    let mut response_schema = ResponseSchema::Outlook;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local_name = e.local_name();
                let field = hashify::tiny_map_ignore_case!(local_name.as_ref(),
                    b"EMailAddress" => RequestField::EmailAddress,
                    b"AcceptableResponseSchema" => RequestField::ResponseSchema,
                );

                let value = match reader.read_event_into(&mut value_buf) {
                    Ok(Event::End(_)) => None,
                    Ok(event) => {
                        let value = match event {
                            Event::Text(text) => text
                                .xml_content(XmlVersion::Implicit1_0)
                                .ok()
                                .map(Cow::into_owned),
                            _ => None,
                        };
                        reader
                            .read_to_end_into(e.name(), &mut value_buf)
                            .map_err(|err| {
                                format!("Error at position {}: {:?}", reader.buffer_position(), err)
                            })?;
                        value
                    }
                    Err(err) => {
                        return Err(format!(
                            "Error at position {}: {:?}",
                            reader.buffer_position(),
                            err
                        ));
                    }
                };

                match (field, value) {
                    (Some(RequestField::EmailAddress), Some(value)) => {
                        email = Some(value);
                    }
                    (Some(RequestField::ResponseSchema), Some(value)) => {
                        response_schema = ResponseSchema::parse(&value);
                    }
                    _ => (),
                }
            }
            Ok(Event::End(_) | Event::Eof) => break,
            Ok(_) => (),
            Err(e) => {
                return Err(format!(
                    "Error at position {}: {:?}",
                    reader.buffer_position(),
                    e
                ));
            }
        }
    }

    match email {
        Some(email) if email.contains('@') => Ok(AutodiscoverRequest {
            email: email.trim().to_lowercase(),
            response_schema,
        }),
        _ => Err(format!(
            "Expected email address, found unexpected value at position {}.",
            reader.buffer_position()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{AutodiscoverRequest, ResponseSchema, parse_autodiscover_request};

    #[test]
    fn parse_autodiscover() {
        const OUTLOOK: &str =
            "http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a";
        const MOBILESYNC: &str =
            "http://schemas.microsoft.com/exchange/autodiscover/mobilesync/responseschema/2006";

        for (request, expected) in [
            (
                format!(
                    r#"<?xml version="1.0" encoding="utf-8"?>
            <Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/outlook/requestschema/2006">
                <Request>
                        <EMailAddress>Email@Example.com</EMailAddress>
                        <AcceptableResponseSchema>{OUTLOOK}</AcceptableResponseSchema>
                </Request>
            </Autodiscover>"#
                ),
                ResponseSchema::Outlook,
            ),
            (
                format!(
                    r#"<Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/outlook/requestschema/2006">
                <Request>
                        <AcceptableResponseSchema>{OUTLOOK}</AcceptableResponseSchema>
                        <EMailAddress>email@example.com</EMailAddress>
                </Request>
            </Autodiscover>"#
                ),
                ResponseSchema::Outlook,
            ),
            (
                r#"<Autodiscover>
                <Request>
                        <EMailAddress>email@example.com</EMailAddress>
                </Request>
            </Autodiscover>"#
                    .to_string(),
                ResponseSchema::Outlook,
            ),
            (
                format!(
                    r#"<?xml version="1.0" encoding="utf-8"?>
            <Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/mobilesync/requestschema/2006">
                <Request>
                        <EMailAddress>email@example.com</EMailAddress>
                        <AcceptableResponseSchema>{MOBILESYNC}</AcceptableResponseSchema>
                </Request>
            </Autodiscover>"#
                ),
                ResponseSchema::MobileSync,
            ),
            (
                format!(
                    r#"<Autodiscover>
                <Request>
                        <LegacyDN>/o=Example/ou=Users/cn=email</LegacyDN>
                        <Unknown><Nested>value</Nested><Empty/></Unknown>
                        <AcceptableResponseSchema>{MOBILESYNC}</AcceptableResponseSchema>
                        <EMailAddress>email@example.com</EMailAddress>
                </Request>
            </Autodiscover>"#
                ),
                ResponseSchema::MobileSync,
            ),
            (
                // [MS-ASCMD] 4.2.1, as printed: the schema wrapped over lines.
                r#"<?xml version="1.0" encoding="utf-8"?>
    <Autodiscover
    xmlns="http://schemas.microsoft.com/exchange/autodiscover/mobilesync/requestschema/2006">
       <Request>
          <EMailAddress>email@example.com</EMailAddress>
          <AcceptableResponseSchema>
          http://schemas.microsoft.com/exchange/autodiscover/mobilesync/
          responseschema/2006
          </AcceptableResponseSchema>
       </Request>
    </Autodiscover>"#
                    .to_string(),
                ResponseSchema::MobileSync,
            ),
            (
                r#"<Autodiscover><Request><EMailAddress>email@example.com</EMailAddress>
                <AcceptableResponseSchema>urn:other</AcceptableResponseSchema></Request></Autodiscover>"#
                    .to_string(),
                ResponseSchema::Unsupported,
            ),
        ] {
            assert_eq!(
                parse_autodiscover_request(request.as_bytes()).expect("valid request"),
                AutodiscoverRequest {
                    email: "email@example.com".to_string(),
                    response_schema: expected,
                },
                "{request}"
            );
        }

        for request in [
            "",
            "<Autodiscover><Request></Request></Autodiscover>",
            "<Autodiscover><Request><EMailAddress>no-domain</EMailAddress></Request></Autodiscover>",
            "<Autodiscover><Request><EMailAddress>email@example.com</Request></Autodiscover>",
            "<Request><EMailAddress>email@example.com</EMailAddress></Request>",
        ] {
            assert!(
                parse_autodiscover_request(request.as_bytes()).is_err(),
                "{request}"
            );
        }
    }

    #[test]
    fn mobilesync_answer() {
        let xml = super::build_mobilesync_response("a&b@example.org", "mail.example.org");
        assert!(xml.contains("<Type>MobileSync</Type>"), "{xml}");
        assert!(
            xml.contains("<Url>https://mail.example.org/Microsoft-Server-ActiveSync</Url>"),
            "{xml}"
        );
        assert!(xml.contains("a&amp;b@example.org"), "{xml}");
        let error = super::build_mobilesync_error("x@elsewhere.test");
        assert!(error.contains("<Status>2</Status>"), "{error}");
        assert!(!error.contains("MobileSync</Type>"), "{error}");
    }

    #[test]
    fn autodiscover_escapes_address() {
        use utils::map::vec_map::VecMap;

        let xml = super::build_autodiscover_response(
            "<b x=\"y\">z</b>@example.org",
            "mail.example.org",
            &VecMap::new(),
            |_| false,
        );
        assert!(!xml.contains("<b x="), "{xml}");
        assert!(
            xml.contains("&lt;b x=&quot;y&quot;&gt;z&lt;/b&gt;@example.org"),
            "{xml}"
        );
    }

    #[test]
    fn autodiscover_encryption() {
        use registry::schema::{enums::ServiceProtocol, structs::Service};
        use utils::map::vec_map::VecMap;

        fn tag<'x>(block: &'x str, name: &str) -> &'x str {
            block
                .split_once(&format!("<{name}>"))
                .and_then(|(_, rest)| rest.split_once(&format!("</{name}>")))
                .map(|(value, _)| value)
                .unwrap()
        }

        for (cleartext, expected) in [
            (
                false,
                vec![
                    ("IMAP", "993", "on", "SSL"),
                    ("POP3", "995", "on", "SSL"),
                    ("SMTP", "465", "on", "SSL"),
                ],
            ),
            (
                true,
                vec![
                    ("IMAP", "993", "on", "SSL"),
                    ("IMAP", "143", "off", "TLS"),
                    ("POP3", "995", "on", "SSL"),
                    ("POP3", "110", "off", "TLS"),
                    ("SMTP", "465", "on", "SSL"),
                    ("SMTP", "587", "off", "TLS"),
                ],
            ),
        ] {
            let services: VecMap<ServiceProtocol, Service> = [
                ServiceProtocol::Imap,
                ServiceProtocol::Pop3,
                ServiceProtocol::Smtp,
                ServiceProtocol::Jmap,
            ]
            .into_iter()
            .map(|protocol| {
                (
                    protocol,
                    Service {
                        hostname: None,
                        cleartext,
                    },
                )
            })
            .collect();
            let response = super::build_autodiscover_response(
                "user@example.com",
                "mail.example.com",
                &services,
                |_| false,
            );

            assert_eq!(
                response
                    .split("<Protocol>")
                    .skip(1)
                    .map(|block| (
                        tag(block, "Type"),
                        tag(block, "Port"),
                        tag(block, "SSL"),
                        tag(block, "Encryption"),
                    ))
                    .collect::<Vec<_>>(),
                expected,
                "cleartext: {cleartext}"
            );
        }
    }
}
