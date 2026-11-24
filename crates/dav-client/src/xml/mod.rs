//! XML for DAV: reading responses into a namespace-resolved tree
//! ([`dom`]), interpreting multistatus and error bodies ([`multistatus`]),
//! and writing request bodies ([`request`]).

pub(crate) mod dom;
pub(crate) mod multistatus;
pub(crate) mod request;
