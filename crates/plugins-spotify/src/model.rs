//! The Spotify Web API response models.

use serde::Deserialize;

#[derive(Deserialize)]
pub struct Track {
    pub(super) name: String,
    pub(super) artists: Vec<ArtistSimple>,
    pub(super) album: AlbumSimple,
    pub(super) external_urls: ExternalUrls,
}

#[derive(Deserialize)]
pub struct Album {
    pub(super) name: String,
    pub(super) artists: Vec<ArtistSimple>,
    pub(super) external_urls: ExternalUrls,
}

#[derive(Deserialize)]
pub struct Artist {
    pub(super) name: String,
    pub(super) genres: Vec<String>,
    pub(super) followers: Followers,
    pub(super) external_urls: ExternalUrls,
}

#[derive(Deserialize)]
pub struct Playlist {
    pub(super) name: String,
    pub(super) owner: Owner,
    pub(super) followers: Followers,
    pub(super) tracks: PlaylistTracks,
    pub(super) external_urls: ExternalUrls,
}

#[derive(Deserialize)]
pub struct ArtistSimple {
    pub(super) name: String,
}

#[derive(Deserialize)]
pub struct AlbumSimple {
    pub(super) name: String,
}

#[derive(Deserialize)]
pub struct ExternalUrls {
    pub(super) spotify: String,
}

#[derive(Deserialize)]
pub struct Followers {
    pub(super) total: u64,
}

#[derive(Deserialize)]
pub struct Owner {
    pub(super) display_name: Option<String>,
    pub(super) id: String,
}

#[derive(Deserialize)]
pub struct PlaylistTracks {
    pub(super) total: u64,
}
