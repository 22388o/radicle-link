// Copyright © 2021 The Radicle Link Contributors
//
// This file is part of radicle-link, distributed under the GPLv3 with Radicle
// Linking Exception. For full terms see the included LICENSE file.

use std::{
    collections::BTreeSet,
    fmt::{self, Debug},
    hash::{Hash, Hasher},
    marker::PhantomData,
};

use bstr::BString;
use either::Either;
use git_ref_format::{Qualified, RefStr};
use link_crypto::PeerId;
use link_git::protocol::{ObjectId, Ref};
use radicle_data::NonEmptyVec;

use crate::{refs, Odb, Refdb};

pub mod error {
    use git_ref_format::RefString;
    use thiserror::Error;

    #[derive(Debug, Error)]
    #[non_exhaustive]
    pub enum WantsHaves<T: std::error::Error + Send + Sync + 'static> {
        #[error("failed to look up ref")]
        Find(#[from] T),

        #[error("malformed ref '{0}'")]
        Malformed(RefString),
    }
}

#[async_trait(?Send)]
pub trait Net {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn run_ls_refs(&self, ls: LsRefs) -> Result<Vec<Ref>, Self::Error>;
    async fn run_fetch(
        &self,
        max_pack_bytes: u64,
        wants: NonEmptyVec<ObjectId>,
        haves: Vec<ObjectId>,
    ) -> Result<(), Self::Error>;
}

pub trait Negotiation<T = Self> {
    /// If and how to perform `ls-refs`.
    fn ls_refs(&self) -> Option<LsRefs>;

    /// Filter a remote-advertised [`Ref`].
    ///
    /// Return `Some` if the ref should be considered, `None` otherwise. This
    /// method may be called with the response of `ls-refs`, the `wanted-refs`
    /// of a `fetch` response, or both.
    fn ref_filter(&self, r: Ref) -> Option<FilteredRef<T>>;

    /// Assemble the `want`s and `have`s for a `fetch`, retaining the refs which
    /// would need updating after the `fetch` succeeds.
    ///
    /// The `refs` are the advertised refs from executing `ls-refs`, filtered
    /// through [`Negotiation::ref_filter`].
    fn wants_haves<R>(
        &self,
        db: &R,
        refs: &[FilteredRef<T>],
    ) -> Result<Option<WantsHaves>, error::WantsHaves<R::FindError>>
    where
        R: Refdb + Odb;

    /// Maximum number of bytes the fetched packfile is allowed to have.
    fn fetch_limit(&self) -> u64;
}

#[derive(Debug)]
pub enum LsRefs {
    /// Do not send ref prefixes, causing the other side to advertise all refs.
    ///
    /// This is provided mainly for completeness.
    Full,
    /// Send ref prefixes.
    Prefix { prefixes: NonEmptyVec<RefPrefix> },
}

impl From<NonEmptyVec<RefPrefix>> for LsRefs {
    fn from(prefixes: NonEmptyVec<RefPrefix>) -> Self {
        Self::Prefix { prefixes }
    }
}

#[derive(Debug)]
pub struct RefPrefix(String);

impl RefPrefix {
    pub fn from_prefix(scope: Option<&PeerId>, prefix: refs::Prefix) -> Self {
        let inner = match scope {
            None => prefix.as_str().to_owned(),
            Some(id) => [
                "refs",
                "remotes",
                refs::from_peer_id(id).as_str(),
                prefix
                    .as_str()
                    .strip_prefix("refs/")
                    .expect("prefix starts with 'refs/'"),
            ]
            .join("/"),
        };

        Self(inner)
    }

    pub fn matches<R: AsRef<RefStr>>(&self, name: R) -> bool {
        name.as_ref().starts_with(self.0.as_str())
    }
}

impl From<refs::Scoped<'_, '_>> for RefPrefix {
    fn from(s: refs::Scoped) -> Self {
        Self(Qualified::from(s).into_refstring().into())
    }
}

impl From<RefPrefix> for BString {
    fn from(p: RefPrefix) -> Self {
        BString::from(p.0)
    }
}

pub type WantsHaves = (NonEmptyVec<ObjectId>, Vec<ObjectId>);

#[derive(Default)]
pub struct BuildWantsHaves {
    wants: BTreeSet<ObjectId>,
    haves: BTreeSet<ObjectId>,
}

impl BuildWantsHaves {
    pub fn want(&mut self, oid: ObjectId) {
        self.wants.insert(oid);
    }

    pub fn have(&mut self, oid: ObjectId) {
        self.haves.insert(oid);
    }

    pub fn add<'a, D, I, T: 'a>(&mut self, db: &D, refs: I) -> Result<&mut Self, D::FindError>
    where
        D: Refdb + Odb,
        I: IntoIterator<Item = &'a FilteredRef<T>>,
    {
        refs.into_iter().try_fold(self, |acc, r| {
            let want = match db.refname_to_id(r.to_remote_tracking())? {
                Some(oid) => {
                    let want = oid.as_ref() != r.tip && !db.contains(&r.tip);
                    acc.have(oid.into());
                    want
                },
                None => !db.contains(&r.tip),
            };
            if want {
                debug!("want {}", r.tip);
                acc.want(r.tip);
            }

            Ok(acc)
        })
    }

    pub fn build(self) -> Option<WantsHaves> {
        let haves = self.haves;
        let wants = self.wants.into_iter().filter(|want| !haves.contains(want));
        NonEmptyVec::from_vec(wants.collect()).map(|wants| (wants, haves.into_iter().collect()))
    }
}

pub struct FilteredRef<T: ?Sized> {
    pub tip: ObjectId,
    pub parsed: refs::Parsed<'static, refs::parsed::Identity>,
    _marker: PhantomData<T>,
}

impl<T> FilteredRef<T> {
    #[allow(clippy::unnecessary_lazy_evaluations)]
    pub fn new(
        tip: ObjectId,
        remote_id: &PeerId,
        parsed: refs::Parsed<refs::parsed::Identity>,
    ) -> Self {
        Self {
            tip,
            parsed: refs::Parsed {
                remote: parsed.remote.or_else(|| Some(*remote_id)),
                inner: parsed.inner.map_right(refs::Owned::into_owned),
            },
            _marker: PhantomData,
        }
    }

    pub fn to_owned<'b>(&self) -> refs::Owned<'b> {
        self.parsed.to_owned()
    }

    pub fn to_remote_tracking<'b>(&self) -> refs::RemoteTracking<'b> {
        self.parsed
            .to_remote_tracking()
            .expect("remote is always set")
    }

    pub fn remote_id(&self) -> &PeerId {
        self.parsed.remote.as_ref().expect("remote is always set")
    }

    pub fn is(&self, rad: &refs::parsed::Rad<refs::parsed::Identity>) -> bool {
        matches!(&self.parsed.inner, Either::Left(r) if r == rad)
    }
}

impl<T> Clone for FilteredRef<T> {
    fn clone(&self) -> Self {
        Self {
            tip: self.tip,
            parsed: self.parsed.clone(),
            _marker: PhantomData,
        }
    }
}

impl<T> Debug for FilteredRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("FilteredRef")
            .field("tip", &self.tip)
            .field("parsed", &self.parsed)
            .finish()
    }
}

impl<T> PartialEq for FilteredRef<T> {
    fn eq(&self, other: &Self) -> bool {
        self.tip == other.tip && self.parsed == other.parsed
    }
}

impl<T> Eq for FilteredRef<T> {}

impl<T> Hash for FilteredRef<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.tip.hash(state);
        self.parsed.hash(state)
    }
}
