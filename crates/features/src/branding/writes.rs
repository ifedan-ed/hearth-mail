/*
 * SPDX-FileCopyrightText: 2026 Coffey Labs LLC
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! What a registry write may set for a logo (BT-3). Only a changed value is checked, so data from before the fork
//! never blocks an unrelated change (BT-4).

use crate::branding::logo;
use jmap_proto::error::set::SetError;
use registry::schema::prelude::{Object, ObjectInner, Property};

/// The logo fields an object carries, with their checks.
fn fields(inner: &ObjectInner) -> Vec<(Property, Option<&str>, fn(&str) -> Result<(), String>)> {
    match inner {
        ObjectInner::Enterprise(o) => vec![
            (Property::LogoUrl, o.logo_url.as_deref(), logo::check),
            (
                Property::LogoUrlDark,
                o.logo_url_dark.as_deref(),
                logo::check,
            ),
        ],
        ObjectInner::Domain(o) => vec![
            (Property::Logo, o.logo.as_deref(), logo::check),
            (Property::LogoDark, o.logo_dark.as_deref(), logo::check),
        ],
        ObjectInner::Tenant(o) => vec![
            (Property::Logo, o.logo.as_deref(), logo::check),
            (Property::LogoDark, o.logo_dark.as_deref(), logo::check),
        ],
        ObjectInner::OAuthClient(o) => vec![(Property::Logo, o.logo.as_deref(), logo::check)],
        _ => vec![],
    }
}

/// Refuses a new or changed logo or template that breaks its rules, with
/// `invalidProperties` naming the field.
pub fn check(old: Option<&Object>, new: &Object) -> Result<(), SetError<Property>> {
    let before = old.map(|old| fields(&old.inner)).unwrap_or_default();
    for (property, value, check) in fields(&new.inner) {
        let Some(value) = value else { continue };
        let unchanged = before
            .iter()
            .any(|(p, v, _)| *p == property && *v == Some(value));
        if unchanged {
            continue;
        }
        if let Err(err) = check(value) {
            return Err(SetError::invalid_properties()
                .with_property(property)
                .with_description(err));
        }
    }
    Ok(())
}
