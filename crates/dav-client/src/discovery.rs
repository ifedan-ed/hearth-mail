//! Finding a person's calendars and address books: the principal
//! (RFC 5397), its calendar and address book homes (RFC 4791 section 6.2.1,
//! RFC 6352 section 7.1.1), and the collections in each home.

use reqwest::Url;

use crate::{
    DavClient, DavError,
    client::{DavMethod, Depth},
    error::protocol,
    href,
    xml::{
        multistatus::{Props, Response},
        request,
    },
};

/// Where a person's calendars and address books live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Homes {
    /// The person's principal (`current-user-principal`).
    pub principal: Url,
    /// The calendar home, when the server offers CalDAV.
    pub calendar_home: Option<Url>,
    /// The address book home, when the server offers CardDAV.
    pub addressbook_home: Option<Url>,
}

/// Whether a collection holds calendar objects or vCards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CollectionKind {
    /// A CalDAV calendar (events, tasks or journals; see [`Components`]).
    Calendar,
    /// A CardDAV address book.
    AddressBook,
}

impl CollectionKind {
    /// The media type of the collection's members, for PUT.
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Calendar => "text/calendar; charset=utf-8",
            Self::AddressBook => "text/vcard; charset=utf-8",
        }
    }
}

/// The calendar components a calendar accepts
/// (`supported-calendar-component-set`, RFC 4791 section 5.2.3). A Nextcloud
/// calendar commonly holds both events and tasks, so it becomes both an
/// ActiveSync calendar folder and a task folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Components {
    /// VEVENT.
    pub event: bool,
    /// VTODO.
    pub todo: bool,
    /// VJOURNAL.
    pub journal: bool,
}

impl Components {
    /// None of them.
    pub const NONE: Self = Self {
        event: false,
        todo: false,
        journal: false,
    };
    /// Every component; what a calendar without the property accepts
    /// (RFC 4791 section 5.2.3: "any calendar component").
    pub const ALL: Self = Self {
        event: true,
        todo: true,
        journal: true,
    };
    /// Journals only: the "Notes" calendar the bridge creates.
    pub const JOURNAL: Self = Self {
        journal: true,
        ..Self::NONE
    };
}

/// A calendar or address book.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    /// The collection's URL (canonical, see [`href`]).
    pub href: Url,
    /// Calendar or address book.
    pub kind: CollectionKind,
    /// The display name, when set.
    pub display_name: Option<String>,
    /// What it holds; [`Components::NONE`] for an address book.
    pub components: Components,
    /// `CS:getctag`, a change marker older servers offer.
    pub ctag: Option<String>,
    /// `DAV:sync-token` (RFC 6578 section 4), the marker sync uses.
    pub sync_token: Option<String>,
    /// The Apple `calendar-color` (`#RRGGBB` or `#RRGGBBAA`).
    pub color: Option<String>,
    /// Whether the person may change its members, from
    /// `current-user-privilege-set` (RFC 3744); `None` when the server
    /// doesn't say. Shared read-only calendars and Nextcloud's birthday
    /// calendar are `Some(false)`.
    pub writable: Option<bool>,
}

/// A collection's change markers, the cheap check a waiting Ping repeats.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChangeMarkers {
    /// `DAV:sync-token`.
    pub sync_token: Option<String>,
    /// `CS:getctag`.
    pub ctag: Option<String>,
}

impl DavClient {
    /// Finds the principal and its homes, starting at the DAV root: one
    /// PROPFIND when the root answers everything, two when (as on Nextcloud)
    /// the home sets are only on the principal.
    pub async fn discover(&self) -> Result<Homes, DavError> {
        let (url, mut root) = self
            .propfind_one(self.base_url(), request::PROPFIND_PRINCIPAL)
            .await?;
        let principal = root
            .props
            .current_user_principal
            .take()
            .ok_or_else(|| protocol("the DAV root names no current-user-principal"))?;
        let principal = href::resolve(&url, &principal)?;
        if root.props.calendar_home_set.is_some() || root.props.addressbook_home_set.is_some() {
            return homes(principal, &url, root.props);
        }
        let (url, on_principal) = self
            .propfind_one(&principal, request::PROPFIND_HOMES)
            .await?;
        homes(principal, &url, on_principal.props)
    }

    /// The calendars or address books in `home` (a PROPFIND of depth 1).
    /// Other members (Nextcloud's inbox, outbox, trash bin, subscriptions)
    /// are left out.
    pub async fn list_collections(&self, home: &Url) -> Result<Vec<Collection>, DavError> {
        let (url, multistatus) = self
            .multistatus(
                DavMethod::Propfind,
                home,
                Depth::One,
                request::PROPFIND_COLLECTIONS,
            )
            .await?;
        let mut collections = Vec::with_capacity(multistatus.responses.len());
        for response in multistatus.responses {
            if let Some(collection) = collection(&url, response)? {
                collections.push(collection);
            }
        }
        Ok(collections)
    }

    /// The change markers of one collection (a PROPFIND of depth 0).
    pub async fn change_markers(&self, collection: &Url) -> Result<ChangeMarkers, DavError> {
        let (_, response) = self
            .propfind_one(collection, request::PROPFIND_TOKENS)
            .await?;
        Ok(ChangeMarkers {
            sync_token: response.props.sync_token,
            ctag: response.props.ctag,
        })
    }

    /// A PROPFIND of depth 0, returning the response for `url` itself.
    pub(crate) async fn propfind_one(
        &self,
        url: &Url,
        body: &'static str,
    ) -> Result<(Url, Response), DavError> {
        let (reply_url, multistatus) = self
            .multistatus(DavMethod::Propfind, url, Depth::Zero, body)
            .await?;
        let response = multistatus
            .responses
            .into_iter()
            .next()
            .ok_or_else(|| protocol("an empty multistatus for a depth 0 PROPFIND"))?;
        Ok((reply_url, response))
    }
}

/// Resolves the home sets found in `props`.
pub(crate) fn homes(principal: Url, base: &Url, props: Props) -> Result<Homes, DavError> {
    let resolve = |href: Option<String>| href.map(|href| href::resolve(base, &href)).transpose();
    Ok(Homes {
        principal,
        calendar_home: resolve(props.calendar_home_set)?,
        addressbook_home: resolve(props.addressbook_home_set)?,
    })
}

/// The collection a PROPFIND response describes, if it is a calendar or an
/// address book.
pub(crate) fn collection(base: &Url, response: Response) -> Result<Option<Collection>, DavError> {
    let Some(resource_type) = response.props.resource_type else {
        return Ok(None);
    };
    let kind = match (resource_type.calendar, resource_type.addressbook) {
        (true, _) => CollectionKind::Calendar,
        (false, true) => CollectionKind::AddressBook,
        (false, false) => return Ok(None),
    };
    let Some(href) = response.hrefs.first() else {
        return Ok(None);
    };
    let props = response.props;
    let components = match kind {
        CollectionKind::Calendar => props.components.unwrap_or(Components::ALL),
        CollectionKind::AddressBook => Components::NONE,
    };
    Ok(Some(Collection {
        href: href::resolve(base, href)?,
        kind,
        display_name: props.display_name.filter(|name| !name.is_empty()),
        components,
        ctag: props.ctag,
        sync_token: props.sync_token,
        color: props.color.filter(|color| !color.is_empty()),
        writable: props.writable,
    }))
}
