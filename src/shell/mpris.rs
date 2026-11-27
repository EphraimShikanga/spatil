//! Now playing over MPRIS on the session bus: the crown's data, and its play, next and volume requests.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use futures_util::stream::StreamExt;
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::calloop::channel::{Event, Sender, channel};
use zbus::fdo::DBusProxy;
use zbus::message::Type as MessageType;
use zbus::proxy::CacheProperties;
use zbus::zvariant::OwnedValue;
use zbus::{Connection, MatchRule, MessageStream, proxy};

use crate::shell::tray::TrayPixmap;
use crate::state::SpatilState;

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const PATH: &str = "/org/mpris/MediaPlayer2";

#[proxy(interface = "org.mpris.MediaPlayer2.Player", default_path = "/org/mpris/MediaPlayer2")]
trait Player {
    fn play_pause(&self) -> zbus::Result<()>;
    fn next(&self) -> zbus::Result<()>;
    #[zbus(property)]
    fn playback_status(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn metadata(&self) -> zbus::Result<HashMap<String, OwnedValue>>;
    #[zbus(property)]
    fn position(&self) -> zbus::Result<i64>;
    #[zbus(property)]
    fn rate(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn volume(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn set_volume(&self, volume: f64) -> zbus::Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Playback {
    Playing,
    Paused,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub art_url: String,
    pub length_us: i64,
}

#[derive(Debug, Clone)]
pub struct NowPlaying {
    pub bus: String,
    pub track: Track,
    pub playback: Playback,
    pub position_us: i64,
    pub rate: f64,
    pub at: Instant,
}

impl NowPlaying {
    /// Position as a fraction of the track, carried forward from the last read at `rate`.
    pub fn progress(&self) -> f32 {
        if self.track.length_us <= 0 {
            return 0.0;
        }
        let mut position = self.position_us as f64;
        if self.playback == Playback::Playing {
            position += self.at.elapsed().as_secs_f64() * 1e6 * self.rate;
        }
        (position / self.track.length_us as f64).clamp(0.0, 1.0) as f32
    }

    pub fn report(&self) -> serde_json::Value {
        serde_json::json!({
            "bus": self.bus,
            "title": self.track.title,
            "artist": self.track.artist,
            "art_url": self.track.art_url,
            "playing": self.playback == Playback::Playing,
            "progress": self.progress(),
        })
    }
}

pub enum MprisRequest {
    PlayPause(String),
    Next(String),
    Volume(String, f64),
}

fn text(meta: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    meta.get(key).and_then(|v| String::try_from(v.try_clone().ok()?).ok())
}

pub fn parse_track(meta: &HashMap<String, OwnedValue>) -> Track {
    let artist = meta
        .get("xesam:artist")
        .and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok())
        .map(|names| names.join(", "))
        .or_else(|| text(meta, "xesam:artist"))
        .unwrap_or_default();
    let length_us = meta
        .get("mpris:length")
        .and_then(|v| {
            let v = v.try_clone().ok()?;
            i64::try_from(v.try_clone().ok()?)
                .ok()
                .or_else(|| u64::try_from(v).ok().map(|u| u.min(i64::MAX as u64) as i64))
        })
        .unwrap_or(0);
    Track {
        title: crate::text::sanitize(&text(meta, "xesam:title").unwrap_or_default()),
        artist: crate::text::sanitize(&artist),
        art_url: text(meta, "mpris:artUrl").unwrap_or_default(),
        length_us,
    }
}

// Q: MPRIS art arrives as a URL (mpris:artUrl). We only have png + resvg decoders, and no HTTP
//    client. Spotify and browsers usually hand out https:// or JPEG files. How far should I go?
//    (file:// PNG/SVG only / add JPEG decoding / JPEG + HTTP fetch)
// A: "where this should be, just create the function or whatever it should be then paste this
//    question and answer there as comments and leave it we will deal with it later"
pub fn load_art(url: &str) -> Option<TrayPixmap> {
    let _ = url;
    None
}

pub fn init_mpris(loop_handle: &LoopHandle<'static, SpatilState>) -> Option<std::sync::mpsc::Sender<MprisRequest>> {
    let (tx, rx) = channel::<Option<NowPlaying>>();
    let inserted = loop_handle.insert_source(rx, |event, _, state: &mut SpatilState| {
        if let Event::Msg(now) = event {
            state.set_now_playing(now);
        }
    });
    if let Err(err) = inserted {
        tracing::warn!("could not insert the MPRIS event source: {err}");
        return None;
    }
    let (requests, incoming) = std::sync::mpsc::channel();
    if let Err(err) = std::thread::Builder::new()
        .name("spatil-mpris".into())
        .spawn(move || run(tx, incoming))
    {
        tracing::warn!("could not start the MPRIS thread: {err}");
        return None;
    }
    Some(requests)
}

fn run(events: Sender<Option<NowPlaying>>, requests: Receiver<MprisRequest>) {
    pollster::block_on(async move {
        let connection = match Connection::session().await {
            Ok(connection) => connection,
            Err(err) => {
                tracing::info!("no session bus ({err}); now playing is unavailable");
                return;
            }
        };
        spawn_request_worker(connection.clone(), requests);
        if let Err(err) = watch(&connection, events).await {
            tracing::warn!("MPRIS watch ended: {err}");
        }
    });
}

async fn watch(connection: &Connection, events: Sender<Option<NowPlaying>>) -> zbus::Result<()> {
    let _ = events.send(current(connection).await);

    let changed = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface("org.freedesktop.DBus.Properties")?
        .member("PropertiesChanged")?
        .path(PATH)?
        .build();
    let seeked = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface("org.mpris.MediaPlayer2.Player")?
        .member("Seeked")?
        .build();
    let mut changes = MessageStream::for_match_rule(changed, connection, None).await?;
    let mut seeks = MessageStream::for_match_rule(seeked, connection, None).await?;
    let mut owners = DBusProxy::new(connection).await?.receive_name_owner_changed().await?;

    loop {
        futures_util::select! {
            message = changes.next() => if message.is_none() { break },
            message = seeks.next() => if message.is_none() { break },
            signal = owners.next() => {
                let Some(signal) = signal else { break };
                let Ok(args) = signal.args() else { continue };
                if !args.name.starts_with(PREFIX) {
                    continue;
                }
            }
            complete => break,
        }
        let _ = events.send(current(connection).await);
    }
    Ok(())
}

/// The player worth showing: one that is playing, else one that is paused.
async fn current(connection: &Connection) -> Option<NowPlaying> {
    let names = DBusProxy::new(connection).await.ok()?.list_names().await.ok()?;
    let mut best: Option<NowPlaying> = None;
    let mut players: Vec<String> = names
        .iter()
        .map(|n| n.to_string())
        // playerctld mirrors whichever player is active; the real one is listed too.
        .filter(|n| n.starts_with(PREFIX) && !n.ends_with(".playerctld"))
        .collect();
    players.sort();
    for name in players {
        let Some(now) = read_player(connection, &name).await else {
            continue;
        };
        let better = match &best {
            None => true,
            Some(b) => b.playback == Playback::Paused && now.playback == Playback::Playing,
        };
        if better {
            best = Some(now);
        }
    }
    best
}

async fn player(connection: &Connection, bus: &str) -> Option<PlayerProxy<'static>> {
    PlayerProxy::builder(connection)
        .destination(bus.to_string())
        .ok()?
        .cache_properties(CacheProperties::No)
        .build()
        .await
        .ok()
}

async fn read_player(connection: &Connection, bus: &str) -> Option<NowPlaying> {
    let proxy = player(connection, bus).await?;
    let playback = match proxy.playback_status().await.ok()?.as_str() {
        "Playing" => Playback::Playing,
        "Paused" => Playback::Paused,
        _ => return None,
    };
    let track = proxy.metadata().await.map(|m| parse_track(&m)).unwrap_or_default();
    Some(NowPlaying {
        bus: bus.to_string(),
        track,
        playback,
        position_us: proxy.position().await.unwrap_or(0),
        rate: proxy.rate().await.unwrap_or(1.0),
        at: Instant::now(),
    })
}

fn spawn_request_worker(connection: Connection, requests: Receiver<MprisRequest>) {
    std::thread::Builder::new()
        .name("spatil-mpris-rpc".into())
        .spawn(move || {
            while let Ok(request) = requests.recv() {
                pollster::block_on(dispatch(&connection, request));
            }
        })
        .map(|_| ())
        .unwrap_or_else(|err| tracing::warn!("could not start the MPRIS request worker: {err}"));
}

async fn dispatch(connection: &Connection, request: MprisRequest) {
    let bus = match &request {
        MprisRequest::PlayPause(bus) | MprisRequest::Next(bus) | MprisRequest::Volume(bus, _) => bus,
    };
    let Some(proxy) = player(connection, bus).await else {
        return;
    };
    let result = match request {
        MprisRequest::PlayPause(_) => proxy.play_pause().await,
        MprisRequest::Next(_) => proxy.next().await,
        MprisRequest::Volume(_, delta) => match proxy.volume().await {
            Ok(volume) => proxy.set_volume((volume + delta).clamp(0.0, 1.0)).await,
            Err(err) => Err(err),
        },
    };
    if let Err(err) = result {
        tracing::debug!("MPRIS request to {bus} failed: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    fn owned(v: Value<'_>) -> OwnedValue {
        OwnedValue::try_from(v).expect("owned")
    }

    #[test]
    fn metadata_yields_title_artists_art_and_length() {
        let mut meta = HashMap::new();
        meta.insert("xesam:title".to_string(), owned(Value::from("Slow Afternoon")));
        meta.insert(
            "xesam:artist".to_string(),
            owned(Value::from(vec!["Lua Ensemble".to_string(), "Guest".to_string()])),
        );
        meta.insert("mpris:artUrl".to_string(), owned(Value::from("file:///tmp/art.png")));
        meta.insert("mpris:length".to_string(), owned(Value::from(200_000_000i64)));
        let track = parse_track(&meta);
        assert_eq!(track.title, "Slow Afternoon");
        assert_eq!(track.artist, "Lua Ensemble, Guest");
        assert_eq!(track.art_url, "file:///tmp/art.png");
        assert_eq!(track.length_us, 200_000_000);
    }

    #[test]
    fn missing_or_odd_metadata_degrades_to_empty() {
        let mut meta = HashMap::new();
        meta.insert("mpris:length".to_string(), owned(Value::from(5_000_000u64)));
        meta.insert("xesam:artist".to_string(), owned(Value::from("Solo")));
        let track = parse_track(&meta);
        assert_eq!(track.title, "");
        assert_eq!(track.artist, "Solo", "a bare string artist is accepted too");
        assert_eq!(track.length_us, 5_000_000, "u64 lengths are read as well");
    }

    #[test]
    fn progress_is_a_fraction_and_paused_does_not_advance() {
        let now = NowPlaying {
            bus: String::new(),
            track: Track { length_us: 100, ..Track::default() },
            playback: Playback::Paused,
            position_us: 38,
            rate: 1.0,
            at: Instant::now() - std::time::Duration::from_secs(5),
        };
        assert!((now.progress() - 0.38).abs() < 1e-6);
        let unknown = NowPlaying { track: Track::default(), ..now };
        assert_eq!(unknown.progress(), 0.0);
    }
}
