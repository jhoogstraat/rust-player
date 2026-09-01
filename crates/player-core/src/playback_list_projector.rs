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
            (SearchTarget::Album { locator, name }, SearchDetail::Album { revision, tracks }) => (
                PlaybackListSource::Album {
                    locator: locator.clone(),
                    name: name.clone(),
                },
                *revision,
                tracks,
            ),
            (
                SearchTarget::Playlist { locator, name, .. },
                SearchDetail::Playlist { revision, tracks },
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

    /// The current unselected candidate, if catalog data is complete.
    pub fn candidate(&self) -> Option<Arc<PlaybackList>> {
        self.cached.as_ref().map(|cached| Arc::clone(&cached.list))
    }

    /// Align `list` to `playable`, searching at or after its prior cursor
    /// before falling back to the first source-scoped identity match.
    pub fn align_cursor(list: &mut PlaybackList, playable: &Playable) -> bool {
        let matches = |candidate: &Playable| {
            candidate.source == playable.source && candidate.locator == playable.locator
        };
        let index = list.tracks[list.current_index.min(list.tracks.len())..]
            .iter()
            .position(matches)
            .map(|offset| list.current_index.min(list.tracks.len()) + offset)
            .or_else(|| list.tracks.iter().position(matches));
        if let Some(index) = index {
            list.current_index = index;
            true
        } else {
            false
        }
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
                tracks,
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
    fn incomplete_and_empty_listings_clear_only_the_candidate() {
        let mut projector = PlaybackListProjector::default();
        assert!(
            projector
                .project_search(&search(1, vec![playable(Source::Spotify, "one")]))
                .is_some()
        );
        assert!(
            projector
                .project_search(&SearchState::Loading {
                    query: "query".to_string()
                })
                .is_none()
        );
        assert!(projector.project_search(&search(2, vec![])).is_none());
        assert!(
            projector
                .project_search(&SearchState::Failed {
                    query: "query".to_string(),
                    message: "offline".to_string(),
                })
                .is_none()
        );
    }

    #[test]
    fn detail_and_library_listings_use_the_same_projection_path() {
        let track = playable(Source::Spotify, "one");
        let mut projector = PlaybackListProjector::default();
        let detail = SearchDetail::Album {
            revision: CatalogRevision::new(1),
            tracks: vec![track.clone()],
        };
        let detail_list = projector
            .project_detail(
                &SearchTarget::Album {
                    locator: "spotify:album:one".to_string(),
                    name: "One".to_string(),
                },
                Some(&detail),
            )
            .unwrap();
        assert!(matches!(
            detail_list.source,
            PlaybackListSource::Album { .. }
        ));

        let library = LibraryState::Done {
            section: LibrarySection::LikedSongs,
            revision: CatalogRevision::new(2),
            entries: vec![LibraryEntry::Track {
                playable: track,
                played_at_ms: None,
            }],
        };
        let library_list = projector.project_library(&library).unwrap();
        assert_eq!(library_list.source, PlaybackListSource::LikedSongs);
    }

    #[test]
    fn unchanged_library_revision_reuses_before_cloning_entries() {
        let mut projector = PlaybackListProjector::default();
        let library = |title| LibraryState::Done {
            section: LibrarySection::LikedSongs,
            revision: CatalogRevision::new(1),
            entries: vec![LibraryEntry::Track {
                playable: playable(Source::Spotify, title),
                played_at_ms: None,
            }],
        };

        let first = projector.project_library(&library("first")).unwrap();
        let reused = projector.project_library(&library("changed")).unwrap();

        assert!(Arc::ptr_eq(&first, &reused));
        assert_eq!(reused.tracks[0].title, "first");
    }

    #[test]
    fn cursor_alignment_is_source_scoped_and_prefers_the_prior_cursor() {
        let spotify = playable(Source::Spotify, "same");
        let mut list = PlaybackList {
            source: PlaybackListSource::LikedSongs,
            tracks: vec![
                spotify.clone(),
                playable(Source::Spotify, "other"),
                spotify.clone(),
            ]
            .into(),
            current_index: 2,
        };

        assert!(PlaybackListProjector::align_cursor(&mut list, &spotify));
        assert_eq!(list.current_index, 2);
    }
}
