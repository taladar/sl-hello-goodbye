#![deny(unknown_lints)]
#![deny(renamed_and_removed_lints)]
#![forbid(unsafe_code)]
#![deny(deprecated)]
#![forbid(private_interfaces)]
#![forbid(private_bounds)]
#![forbid(non_fmt_panics)]
#![deny(unreachable_code)]
#![deny(unreachable_patterns)]
#![forbid(unused_doc_comments)]
#![forbid(unused_must_use)]
#![deny(while_true)]
#![deny(unused_parens)]
#![deny(redundant_semicolons)]
#![deny(non_ascii_idents)]
#![deny(confusable_idents)]
#![warn(missing_docs)]
#![warn(clippy::missing_docs_in_private_items)]
#![warn(clippy::cargo_common_metadata)]
#![warn(rustdoc::missing_crate_level_docs)]
#![deny(rustdoc::broken_intra_doc_links)]
#![warn(missing_debug_implementations)]
#![deny(clippy::mod_module_files)]
//#![warn(clippy::pedantic)]
#![warn(clippy::redundant_else)]
#![warn(clippy::must_use_candidate)]
#![warn(clippy::missing_panics_doc)]
#![warn(clippy::missing_errors_doc)]
#![warn(clippy::panic)]
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![doc = include_str!("../../README.md")]

use clap::Parser;

use tracing::instrument;
use tracing_subscriber::{
    filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer, Registry,
};

/// Error enum for the application
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// error reading environment variable
    #[error("error when retrieving environment variable: {0}")]
    EnvVarError(#[from] std::env::VarError),
    /// error in clap
    #[error("error in CLI option parsing: {0}")]
    ClapError(#[from] clap::Error),
    /// error parsing log filter
    #[error("error parsing log filter: {0}")]
    LogFilterParseError(#[from] tracing_subscriber::filter::ParseError),
    /// error determining current user home directory
    #[error("error determining current user home directory")]
    HomeDirError,
    /// local chat file not found
    #[error("local chat file not found: {0}")]
    LocalChatFileNotFound(std::path::PathBuf),
    /// error created MuxedLines
    #[error("error creating MuxedLines: {0}")]
    MuxedLinesError(std::io::Error),
    /// error adding file to MuxedLines
    #[error("error adding file to MuxedLines: {0}")]
    MuxedLinesAddFileError(std::io::Error),
}

/// The Clap type for all the commandline parameters
#[derive(clap::Parser, Debug)]
#[clap(name = clap::crate_name!(),
       about = clap::crate_description!(),
       author = clap::crate_authors!(),
       version = clap::crate_version!(),
       )]
struct Options {
    /// name of the logged in avatar whose chat.txt log file to watch (not display name)
    #[clap(long)]
    avatar_name: String,
}

/// represents Second Life region coordinates
#[derive(Debug, Clone)]
pub struct SecondLifeRegionCoordinates {
    /// x
    pub x: i16,
    /// y
    pub y: i16,
    /// z
    pub z: i16,
}

/// represents a Second Life Location
#[derive(Debug, Clone)]
pub struct SecondLifeLocation {
    /// region name
    pub region_name: String,
    /// coordinates
    pub coordinates: SecondLifeRegionCoordinates,
}

/// represents a Second Life system message
#[derive(Debug, Clone)]
pub enum SecondLifeSystemMessage {
    /// message about a saved snapshot
    SavedSnapshotMessage {
        /// the snapshot filename
        filename: std::path::PathBuf,
    },
    /// message about a sent payment
    SentPaymentMessage {
        /// the recipient avatar UUID
        recipient_avatar_key: uuid::Uuid,
        /// the amount paid
        amount: u64,
    },
    /// message about a received payment
    ReceivedPaymentMessage {
        /// the sender avatar UUID
        sender_avatar_key: uuid::Uuid,
        /// the amount received
        amount: u64,
    },
    /// message about a song playing on stream
    NowPlayingMessage {
        /// the song name
        song_name: String,
    },
    /// message about a completed teleport
    TeleportCompletedMessage {
        /// teleported originated at this location
        origin: SecondLifeLocation,
    },
    /// message about a region restart of the region that the avatar is in
    RegionRestartMessage,
    /// message about an object giving the current avatar an object
    ObjectGaveObjectMessage {
        /// the giving object name
        giving_object_name: String,
        /// the giving object location
        giving_object_location: SecondLifeLocation,
        /// the giving object owner
        giving_object_owner: uuid::Uuid,
        /// the name of the given object
        given_object_name: String,
    },
    /// message about an avatar giving the current avatar an object
    AvatarGaveObjectMessage {
        /// the giving avatar name
        giving_avatar_name: String,
        /// the name of the given object
        given_object_name: String,
    },
    /// other system message
    OtherSystemMessage {
        /// the raw message
        message: String,
    },
}

/// represents a Second Life chat volume
#[derive(Debug, Clone)]
pub enum SecondLifeChatVolume {
    /// whisper (10m)
    Whisper,
    /// say (20m, default, a.k.a. chat range)
    Say,
    /// shout (100m)
    Shout,
    /// region say (the whole region)
    RegionSay,
}

/// represents a Second Life avatar related message
#[derive(Debug, Clone)]
pub enum SecondLifeAvatarMessage {
    /// a message about the avatar whispering, saying or shouting something
    Chat {
        /// how "loud" the message was (whisper, say, shout or region say)
        volume: SecondLifeChatVolume,
        /// the chat message
        message: String,
    },
    /// a message about an avatar coming online
    CameOnline,
    /// a message about an avatar going offline
    WentOffline,
    /// a message about an avatar entering chat range
    EnteredChatRange {
        /// the distance where the avatar entered chat range in meters
        distance: Option<f64>,
    },
    /// a message about an avatar leaving chat range
    LeftChatRange,
    /// a message about an avatar entering draw distance
    EnteredDrawDistance {
        /// the distance where the avatar entered draw distance in meters
        distance: Option<f64>,
    },
    /// a message about an avatar leaving draw distance
    LeftDrawDistance,
    /// a message about an avatar entering the region
    EnteredRegion {
        /// the distance where the avatar entered the region in meters
        distance: Option<f64>,
    },
    /// a message about an avatar leaving the region
    LeftRegion,
}

/// represents an event commemorated in the Second Life chat log
#[derive(Debug, Clone)]
pub enum SecondLifeChatLogEvent {
    /// line about an avatar (or an object doing things indistinguishable from an avatar in the chat log)
    AvatarLine {
        /// name of the avatar
        username: String,
        /// message
        message: SecondLifeAvatarMessage,
    },
    /// a message by the Second Life viewer or server itself
    SystemMessage {
        /// the system message
        message: SecondLifeSystemMessage,
    },
}

/// represents a Second Life chat log line
#[derive(Debug, Clone)]
pub struct SecondLifeChatLogLine {
    /// timestamp of the chat log line
    timestamp: time::PrimitiveDateTime,
    /// event that happened at that time
    event: SecondLifeChatLogEvent,
}

/// The main behaviour of the binary should go here
#[instrument]
async fn do_stuff() -> Result<(), crate::Error> {
    let options = Options::parse();
    tracing::debug!("{:#?}", options);

    let avatar_dir_name = options.avatar_name.replace(" ", "_").to_lowercase();
    tracing::debug!("Avatar dir name: {}", avatar_dir_name);

    let Some(home_dir) = dirs2::home_dir() else {
        tracing::error!("Could not determine current user home directory");
        return Err(crate::Error::HomeDirError);
    };

    let avatar_dir = home_dir.join(".firestorm/").join(avatar_dir_name);

    let local_chat_log_file = avatar_dir.join("chat.txt");

    if !local_chat_log_file.exists() {
        tracing::error!(
            "Local chat log {} does not exist for this avatar",
            local_chat_log_file.display()
        );
        return Err(crate::Error::LocalChatFileNotFound(local_chat_log_file));
    }

    let mut lines = linemux::MuxedLines::new().map_err(crate::Error::MuxedLinesError)?;

    lines
        .add_file(local_chat_log_file)
        .await
        .map_err(crate::Error::MuxedLinesAddFileError)?;

    while let Ok(Some(line)) = lines.next_line().await {
        println!("source: {}, line: {}", line.source().display(), line.line());
    }

    Ok(())
}

/// The main function mainly just handles setting up tracing
/// and handling any Err Results.
#[tokio::main]
async fn main() -> Result<(), Error> {
    let terminal_env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::WARN.into())
        .parse(std::env::var("RUST_LOG").unwrap_or_else(|_| "".to_string()))?;
    let file_env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::TRACE.into())
        .parse(std::env::var("SL_HELLO_GOODBYE_LOG").unwrap_or_else(|_| "".to_string()))?;
    let registry = Registry::default();
    let registry =
        registry.with(tracing_subscriber::fmt::Layer::default().with_filter(terminal_env_filter));
    let log_dir = std::env::var("SL_HELLO_GOODBYE_LOG_DIR");
    if let Ok(log_dir) = log_dir {
        let log_file = if let Ok(log_file) = std::env::var("SL_HELLO_GOODBYE_LOG_FILE") {
            log_file
        } else {
            "sl_hello_goodbye.log".to_string()
        };
        tracing::info!("Logging to {}/{}", log_dir, log_file);
        let file_appender = tracing_appender::rolling::never(log_dir, log_file);
        registry
            .with(
                tracing_subscriber::fmt::Layer::default()
                    .with_writer(file_appender)
                    .with_filter(file_env_filter),
            )
            .init();
    } else {
        registry.init();
    }
    log_panics::init();
    match do_stuff().await {
        Ok(_) => (),
        Err(e) => {
            tracing::error!("{}", e);
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
    tracing::debug!("Exiting");
    Ok(())
}

#[cfg(test)]
mod test {
    //use super::*;
    //use pretty_assertions::{assert_eq, assert_ne};
}
