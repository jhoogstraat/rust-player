//! Candidate implicit-list derivation for completed catalog listings.

use std::sync::Arc;

use crate::{
    CatalogRevision, LibraryEntry, LibrarySection, LibraryState, Playable, PlaybackList,
    PlaybackListSource, SearchDetail, SearchState, SearchTarget,
};

/// Derives one candidate list and retains only its current allocation.
#[derive(Default)]
pub struct PlaybackListProjector {
    cached: Option<CachedCandidate>,
}

struct CachedCandidate {
    source: PlaybackListSource,
    revision: CatalogRevision,
    list: Arc<PlaybackList>,
}

impl PlaybackListProjector {
    /// Derive a candidate from a search listing, or clear it for a non-complete
    /// state.
    pub fn project_search(&mut self, state: &SearchState) -> Option<Arc<PlaybackList>> {
        match state {
            SearchState::Done {
                query,
                revision,
                results,
            } => self.project(
                PlaybackListSource::SearchResults {
                    query: query.clone(),
                },
                *revision,
                &results.tracks,
            ),
            SearchState::Idle | SearchState::Loading { .. } | SearchState::Failed { .. } => {
                self.clear()
            }
        }
    }

    /// Derive a candidate from a completed search detail listing.
    pub fn project_detail(
        &mut self,
        target: &SearchTarget,
        detail: Option<&SearchDetail>,
    ) -> Option<Arc<PlaybackList>> {
        let Some(detail) = detail else {
            return self.clear();
        };
        if !detail.matches_target(target) {
            return self.clear();
        }
        if !detail.is_complete() {
            return self.clear();
        }
        let (source, revision, tracks) = match (target, detail) {
            (
                SearchTarget::Artist { locator, name },
                SearchDetail::Artist {
                    revision, tracks, ..
                },
            ) => (
                PlaybackListSource::Artist {
                    locator: locator.clone(),
                    name: name.clone(),
                },
                *revision,
                tracks,
            ),
            (
                SearchTarget::Album { locator, name },
                SearchDetail::Album {
                    revision, tracks, ..
                },
            ) => (
                PlaybackListSource::Album {
                    locator: locator.clone(),
                    name: name.clone(),
                },
                *revision,
                tracks,
            ),
            (
                SearchTarget::Playlist { locator, name, .. },
                SearchDetail::Playlist {
                    revision, tracks, ..
                },
            ) => (
                PlaybackListSource::Playlist {
                    locator: locator.clone(),
                    name: name.clone(),
                },
                *revision,
                tracks,
            ),
            _ => return self.clear(),
        };
        self.project(source, revision, tracks)
    }

    /// Derive a candidate from a library listing, or clear it when its catalog
    /// state is not completed.
    pub fn project_library(&mut self, state: &LibraryState) -> Option<Arc<PlaybackList>> {
        let LibraryState::Done {
            section,
            revision,
            entries,
        } = state
        else {
            return self.clear();
        };
        let source = match section {
            LibrarySection::LikedSongs => PlaybackListSource::LikedSongs,
            LibrarySection::RecentlyPlayed => PlaybackListSource::RecentlyPlayed,
            LibrarySection::Playlists => return self.clear(),
        };
        if let Some(cached) = &self.cached
            && cached.source == source
            && cached.revision == *revision
        {
            return Some(Arc::clone(&cached.list));
        }
        let tracks = entries
            .iter()
            .filter_map(|entry| match entry {
                LibraryEntry::Track { playable, .. } => Some(playable.clone()),
                LibraryEntry::Playlist { .. } => None,
            })
            .collect::<Vec<_>>();
        self.project(source, *revision, &tracks)
    }

    /// Clear the candidate cache. This never changes a selected Playback List.
    pub fn clear(&mut self) -> Option<Arc<PlaybackList>> {
        self.cached = None;
        None
    }

    fn project(
        &mut self,
        source: PlaybackListSource,
        revision: CatalogRevision,
        tracks: &[Playable],
    ) -> Option<Arc<PlaybackList>> {
        if tracks.is_empty() {
            return self.clear();
        }
        if let Some(cached) = &self.cached
            && cached.source == source
            && cached.revision == revision
        {
            return Some(Arc::clone(&cached.list));
        }
        let list = Arc::new(PlaybackList {
            source: source.clone(),
            tracks: tracks.to_vec().into(),
            current_index: 0,
        });
        self.cached = Some(CachedCandidate {
            source,
            revision,
            list: Arc::clone(&list),
        });
        Some(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SearchResults, Source};

    fn playable(source: Source, locator: &str) -> Playable {
        Playable {
            source,
            locator: locator.to_string(),
            title: locator.to_string(),
            artists: vec![],
            album: String::new(),
            duration_ms: 0,
        }
    }

    fn search(revision: u64, tracks: Vec<Playable>) -> SearchState {
        SearchState::Done {
            query: "query".to_string(),
            revision: CatalogRevision::new(revision),
            results: SearchResults {
                tracks: tracks.into(),
                ..SearchResults::default()
            },
        }
    }

    #[test]
    fn same_revision_reuses_and_new_revision_replaces_the_candidate() {
        let mut projector = PlaybackListProjector::default();
        let first = projector
            .project_search(&search(1, vec![playable(Source::Spotify, "one")]))
            .unwrap();
        let reused = projector
            .project_search(&search(1, vec![playable(Source::Spotify, "changed")]))
            .unwrap();
        let replaced = projector
            .project_search(&search(2, vec![playable(Source::Spotify, "two")]))
            .unwrap();

        assert!(Arc::ptr_eq(&first, &reused));
        assert!(!Arc::ptr_eq(&first, &replaced));
        assert_eq!(replaced.tracks[0].locator, "two");
    }

    #[test]
    fn detail_identity_and_completeness_gate_playback_projection() {
        let track = playable(Source::Spotify, "one");
        let target = SearchTarget::Album {
            locator: "spotify:album:b".to_string(),
            name: "B".to_string(),
        };
        let mut projector = PlaybackListProjector::default();
        let stale = SearchDetail::Album {
            target_locator: Some("spotify:album:a".to_string()),
            complete: true,
            revision: CatalogRevision::new(1),
            tracks: vec![track.clone()].into(),
        };
        assert!(projector.project_detail(&target, Some(&stale)).is_none());

        let partial = SearchDetail::Album {
            target_locator: Some("spotify:album:b".to_string()),
            complete: false,
            revision: CatalogRevision::new(2),
            tracks: vec![track].into(),
        };
        assert!(projector.project_detail(&target, Some(&partial)).is_none());
    }
}
