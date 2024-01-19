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

use tracing::instrument;
use tracing_subscriber::{
    filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer, Registry,
};

use ariadne::{Color, Fmt, Label, Report, ReportKind, Source};
use chumsky::{prelude::*, text::digits, Parser};

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
    /// error parsing chat log line
    #[error("error parsing chat log line: {0}")]
    ChatLogLineParseError(ChumskyError),
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

/// a wrapped error in case parsing fails to get proper error output
/// the chumsky errors themselves lack Display and std::error::Error
/// implementations
#[derive(Debug)]
pub struct ChumskyError {
    /// description of the object we were trying to parse
    pub description: String,
    /// source string for parsing
    pub source: String,
    /// errors encountered during parsing
    pub errors: Vec<chumsky::error::Simple<char>>,
}

impl std::fmt::Display for ChumskyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for e in &self.errors {
            let msg = format!(
                "While parsing {}: {}{}, expected {}",
                self.description,
                if e.found().is_some() {
                    "Unexpected token"
                } else {
                    "Unexpected end of input"
                },
                if let Some(label) = e.label() {
                    format!(" while parsing {}", label)
                } else {
                    String::new()
                },
                if e.expected().len() == 0 {
                    "end of input".to_string()
                } else {
                    e.expected()
                        .map(|expected| match expected {
                            Some(expected) => expected.to_string(),
                            None => "end of input".to_string(),
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                },
            );

            let report = Report::build(ReportKind::Error, (), e.span().start)
                .with_code(3)
                .with_message(msg)
                .with_label(
                    Label::new(e.span())
                        .with_message(format!(
                            "Unexpected {}",
                            e.found()
                                .map(|c| format!("token {}", c.fg(Color::Red)))
                                .unwrap_or_else(|| "end of input".to_string())
                        ))
                        .with_color(Color::Red),
                );

            let report = match e.reason() {
                chumsky::error::SimpleReason::Unclosed { span, delimiter } => report.with_label(
                    Label::new(span.clone())
                        .with_message(format!(
                            "Unclosed delimiter {}",
                            delimiter.fg(Color::Yellow)
                        ))
                        .with_color(Color::Yellow),
                ),
                chumsky::error::SimpleReason::Unexpected => report,
                chumsky::error::SimpleReason::Custom(msg) => report.with_label(
                    Label::new(e.span())
                        .with_message(format!("{}", msg.fg(Color::Yellow)))
                        .with_color(Color::Yellow),
                ),
            };

            let mut s: Vec<u8> = Vec::new();
            report
                .finish()
                .write(Source::from(&self.source), &mut s)
                .map_err(|_| <std::fmt::Error as std::default::Default>::default())?;
            let s = std::str::from_utf8(&s).expect("Expected ariadne to generate valid UTF-8");
            write!(f, "{}", s)?;
        }
        Ok(())
    }
}

impl std::error::Error for ChumskyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
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

/// parse a string like "10/20/30" into a SecondLifeRegionCoordinates
///
/// # Errors
///
/// returns an error if the string could not be parsed
pub fn slash_separated_coordinate_parser(
) -> impl Parser<char, SecondLifeRegionCoordinates, Error = Simple<char>> {
    digits(10)
        .separated_by(just('/'))
        .exactly(3)
        .try_map(|x, span: std::ops::Range<usize>| {
            Ok(SecondLifeRegionCoordinates {
                x: x[0]
                    .parse()
                    .map_err(|e| Simple::custom(span.clone(), format!("{:?}", e)))?,
                y: x[1]
                    .parse()
                    .map_err(|e| Simple::custom(span.clone(), format!("{:?}", e)))?,
                z: x[2]
                    .parse()
                    .map_err(|e| Simple::custom(span.clone(), format!("{:?}", e)))?,
            })
        })
}

/// represent a Second Life region name
#[derive(Debug, Clone)]
pub struct SecondLifeRegionName(String);

/// parse a string into a SecondLifeRegionName
///
/// # Errors
///
/// returns an error if the string could not be parsed
pub fn region_name_parser() -> impl Parser<char, SecondLifeRegionName, Error = Simple<char>> {
    text::ident().map(SecondLifeRegionName)
}

/// represents a Second Life Location
#[derive(Debug, Clone)]
pub struct SecondLifeLocation {
    /// region name
    pub region_name: SecondLifeRegionName,
    /// coordinates
    pub coordinates: SecondLifeRegionCoordinates,
}

/// parse a string like "DaBoom/10/20/30" into a SecondLifeLocation
///
/// # Errors
///
/// returns an error if the string could not be parsed
pub fn location_parser() -> impl Parser<char, SecondLifeLocation, Error = Simple<char>> {
    region_name_parser()
        .then_ignore(just('/'))
        .then(slash_separated_coordinate_parser())
        .map(|(region_name, coordinates)| SecondLifeLocation {
            region_name,
            coordinates,
        })
}

/// represents a Second Life avatar key (UUID)
#[derive(Debug, Clone)]
pub struct SecondLifeAvatarKey(uuid::Uuid);

/// parse a UUID
///
/// # Errors
///
/// returns an error if the string could not be parsed
pub fn uuid_parser() -> impl Parser<char, uuid::Uuid, Error = Simple<char>> {
    one_of("0123456789abcdef")
        .repeated()
        .exactly(6)
        .collect::<String>()
        .then_ignore(just('-'))
        .then(
            one_of("0123456789abcdef")
                .repeated()
                .exactly(4)
                .collect::<String>(),
        )
        .then_ignore(just('-'))
        .then(
            one_of("0123456789abcdef")
                .repeated()
                .exactly(4)
                .collect::<String>(),
        )
        .then_ignore(just('-'))
        .then(
            one_of("0123456789abcdef")
                .repeated()
                .exactly(4)
                .collect::<String>(),
        )
        .then_ignore(just('-'))
        .then(
            one_of("0123456789abcdef")
                .repeated()
                .exactly(12)
                .collect::<String>(),
        )
        .try_map(|((((a, b), c), d), e), span: std::ops::Range<usize>| {
            Ok(
                uuid::Uuid::parse_str(&format!("{}-{}-{}-{}-{}", a, b, c, d, e))
                    .map_err(|e| Simple::custom(span.clone(), format!("{:?}", e)))?,
            )
        })
}

/// parse an agent URL into a SecondLifeAvatarKey
///
/// "secondlife:///app/agent/daf2e68d-6ccc-4592-a049-3306e4820821/about"
///
/// # Errors
///
/// returns an error if the string could not be parsed
pub fn agent_url_as_avatar_key_parser(
) -> impl Parser<char, SecondLifeAvatarKey, Error = Simple<char>> {
    just("secondlife:///app/agent/")
        .ignore_then(uuid_parser())
        .then_ignore(just("/about"))
        .map(|uuid| SecondLifeAvatarKey(uuid))
}

/// represents a L$ amount
#[derive(Debug, Clone)]
pub struct SecondLifeLindenAmount(u64);

/// parse a Linden amount
///
/// "L$1234"
///
/// # Errors
///
/// returns an error if the string could not be parsed
pub fn linden_amount_parser() -> impl Parser<char, SecondLifeLindenAmount, Error = Simple<char>> {
    just("L$")
        .ignore_then(digits(10))
        .try_map(|x: String, span: std::ops::Range<usize>| {
            Ok(SecondLifeLindenAmount(x.parse().map_err(|e| {
                Simple::custom(span.clone(), format!("{:?}", e))
            })?))
        })
}

/// represents a Second Life distance in meters
#[derive(Debug, Clone)]
pub struct SecondLifeDistance(f64);

/// parse a distance
///
/// "235.23 m"
///
/// # Errors
///
/// returns an error if the string could not be parsed
pub fn distance_parser() -> impl Parser<char, SecondLifeDistance, Error = Simple<char>> {
    digits(10)
        .then_ignore(just('.'))
        .then(digits(10))
        .then_ignore(just(" m"))
        .try_map(|(full, decimal), span: std::ops::Range<usize>| {
            Ok(SecondLifeDistance(
                format!("{}.{}", full, decimal)
                    .parse()
                    .map_err(|e| Simple::custom(span.clone(), format!("{:?}", e)))?,
            ))
        })
}

/// represents a Second Life system message
#[derive(Debug, Clone)]
pub enum SecondLifeSystemMessage {
    /// message about a saved snapshot
    SavedSnapshotMessage {
        /// the snapshot filename
        filename: std::path::PathBuf,
    },
    /// message about a saved attachment
    AttachmentSavedMessage,
    /// message about a sent payment
    SentPaymentMessage {
        /// the recipient avatar UUID
        recipient_avatar_key: SecondLifeAvatarKey,
        /// the amount paid
        amount: SecondLifeLindenAmount,
    },
    /// message about a received payment
    ReceivedPaymentMessage {
        /// the sender avatar UUID
        sender_avatar_key: SecondLifeAvatarKey,
        /// the amount received
        amount: SecondLifeLindenAmount,
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
        giving_object_owner: SecondLifeAvatarKey,
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

/// parse a Second Life system message
///
/// # Errors
///
/// returns an error if the string could not be parsed
pub fn system_message_parser() -> impl Parser<char, SecondLifeSystemMessage, Error = Simple<char>> {
    // TODO: implement properly
    any()
        .repeated()
        .collect::<String>()
        .try_map(|s, _span: std::ops::Range<usize>| {
            Ok(SecondLifeSystemMessage::OtherSystemMessage { message: s })
        })
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

impl SecondLifeChatVolume {
    /// identify the chat volume of a message and strip it off the message
    pub fn volume_and_message(s: String) -> (SecondLifeChatVolume, String) {
        if s.starts_with("whispers: ") {
            (SecondLifeChatVolume::Whisper, s[10..].to_string())
        } else if s.starts_with("shouts: ") {
            (SecondLifeChatVolume::Shout, s[8..].to_string())
        } else {
            (SecondLifeChatVolume::Say, s)
        }
    }
}

/// represents a Second Life area of significance
#[derive(Debug, Clone)]
pub enum SecondLifeArea {
    /// chat range
    ChatRange,
    /// draw distance
    DrawDistance,
    /// region
    Region,
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
    /// an emote (chat message starting with /me in the log)
    Emote {
        /// how "loud" the message was (whisper, say, shout or region say)
        volume: SecondLifeChatVolume,
        /// the chat message without the /me
        message: String,
    },
    /// a message about an avatar coming online
    CameOnline,
    /// a message about an avatar going offline
    WentOffline,
    /// a message about an avatar entering an area of significance
    EnteredArea {
        /// the area of significance
        area: SecondLifeArea,
        /// the distance where the avatar entered the area
        distance: Option<SecondLifeDistance>,
    },
    /// a message about an avatar leaving an area of significance
    LeftArea {
        /// the area of significance
        area: SecondLifeArea,
    },
}

/// parse a Second Life avatar chat message
///
/// # Errors
///
/// returns an error if the parser fails
fn avatar_chat_message_parser() -> impl Parser<char, SecondLifeAvatarMessage, Error = Simple<char>>
{
    any()
        .repeated()
        .collect::<String>()
        .try_map(|s, _span: std::ops::Range<usize>| {
            let (v, s) = SecondLifeChatVolume::volume_and_message(s.to_string());
            Ok(SecondLifeAvatarMessage::Chat {
                volume: v,
                message: s,
            })
        })
}

/// parse a Second Life avatar emote message
///
/// # Errors
///
/// returns an error if the parser fails
fn avatar_emote_message_parser() -> impl Parser<char, SecondLifeAvatarMessage, Error = Simple<char>>
{
    just("/me ")
        .ignore_then(any().repeated().collect::<String>())
        .try_map(|s, _span: std::ops::Range<usize>| {
            let (v, s) = SecondLifeChatVolume::volume_and_message(s);
            Ok(SecondLifeAvatarMessage::Emote {
                volume: v,
                message: s,
            })
        })
}

/// parse a Second Life avatar message
///
/// # Errors
///
/// returns an error if the parser fails
fn avatar_message_parser() -> impl Parser<char, SecondLifeAvatarMessage, Error = Simple<char>> {
    // TODO: implement properly
    avatar_emote_message_parser().or(avatar_chat_message_parser())
}

/// represents an event commemorated in the Second Life chat log
#[derive(Debug, Clone)]
pub enum SecondLifeChatLogEvent {
    /// line about an avatar (or an object doing things indistinguishable from an avatar in the chat log)
    AvatarLine {
        /// name of the avatar or object
        name: String,
        /// message
        message: SecondLifeAvatarMessage,
    },
    /// a message by the Second Life viewer or server itself
    SystemMessage {
        /// the system message
        message: SecondLifeSystemMessage,
    },
    /// a message without a colon, most likely an unnamed object like a translator, spanker, etc.
    OtherMessage {
        /// the message
        message: String,
    },
}

/// parse a second life avatar name as it appears in the chat log before a message
///
/// # Errors
///
/// returns an error if the parser fails
fn avatar_name_parser() -> impl Parser<char, String, Error = Simple<char>> {
    none_of(":")
        .repeated()
        .collect::<String>()
        .try_map(|s, _span: std::ops::Range<usize>| Ok(s))
}

/// parse a Second Life chat log event
///
/// # Errors
///
/// returns an error if the parser fails
fn chat_log_event_parser() -> impl Parser<char, SecondLifeChatLogEvent, Error = Simple<char>> {
    just("Second Life: ")
        .ignore_then(
            system_message_parser().try_map(|message, _span: std::ops::Range<usize>| {
                Ok(SecondLifeChatLogEvent::SystemMessage { message })
            }),
        )
        .or(avatar_name_parser()
            .then_ignore(just(": "))
            .then(avatar_message_parser())
            .try_map(|(name, message), _span: std::ops::Range<usize>| {
                Ok(SecondLifeChatLogEvent::AvatarLine { name, message })
            }))
        .or(any()
            .repeated()
            .collect::<String>()
            .try_map(|s, _span: std::ops::Range<usize>| {
                Ok(SecondLifeChatLogEvent::OtherMessage { message: s })
            }))
}

/// represents a Second Life chat log line
#[derive(Debug, Clone)]
pub struct SecondLifeChatLogLine {
    /// timestamp of the chat log line
    timestamp: time::PrimitiveDateTime,
    /// event that happened at that time
    event: SecondLifeChatLogEvent,
}

/// parse a Second Life chat log line
///
/// # Errors
///
/// returns an error if the parser fails
fn sl_chat_log_line_parser() -> impl Parser<char, SecondLifeChatLogLine, Error = Simple<char>> {
    just("[")
        .ignore_then(
            one_of("0123456789")
                .repeated()
                .exactly(4)
                .collect::<String>(),
        )
        .then(
            just("/").ignore_then(
                one_of("0123456789")
                    .repeated()
                    .exactly(2)
                    .collect::<String>(),
            ),
        )
        .then(
            just("/").ignore_then(
                one_of("0123456789")
                    .repeated()
                    .exactly(2)
                    .collect::<String>(),
            ),
        )
        .then(
            just(" ").ignore_then(
                one_of("0123456789")
                    .repeated()
                    .exactly(2)
                    .collect::<String>(),
            ),
        )
        .then(
            just(":").ignore_then(
                one_of("0123456789")
                    .repeated()
                    .exactly(2)
                    .collect::<String>(),
            ),
        )
        .then(
            just(":")
                .ignore_then(
                    one_of("0123456789")
                        .repeated()
                        .exactly(2)
                        .collect::<String>(),
                )
                .or_not(),
        )
        .then_ignore(just("]  "))
        .then(chat_log_event_parser())
        .try_map(
            |((((((year, month), day), hour), minute), second), event),
             span: std::ops::Range<usize>| {
                let second = second.unwrap_or("00".to_string());
                let format = time::macros::format_description!(
                    "[year]/[month]/[day] [hour]:[minute]:[second]"
                );
                Ok(SecondLifeChatLogLine {
                    timestamp: time::PrimitiveDateTime::parse(
                        &format!("{}/{}/{} {}:{}:{}", year, month, day, hour, minute, second),
                        format,
                    )
                    .map_err(|e| Simple::custom(span, format!("{:?}", e)))?,
                    event,
                })
            },
        )
}

/// The main behaviour of the binary should go here
#[instrument]
async fn do_stuff() -> Result<(), crate::Error> {
    let options = <Options as clap::Parser>::parse();
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
        let parsed_line = sl_chat_log_line_parser().parse(line.line());
        println!("parse result:\n{:#?}", parsed_line);
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
