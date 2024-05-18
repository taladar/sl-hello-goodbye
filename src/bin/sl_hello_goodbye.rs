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

use std::collections::BTreeMap;
use std::path::PathBuf;

use chumsky::text::whitespace;
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
    /// error joining the log reader task before shutdown
    #[error("error joining the log reader task before shutdown: {0}")]
    JoinError(#[from] tokio::task::JoinError),
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
            let Ok(s) = std::str::from_utf8(&s) else {
                tracing::error!("Expected ariadne to produce valid UTF-8");
                return Err(std::fmt::Error);
            };
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
#[must_use]
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
#[must_use]
pub fn region_name_parser() -> impl Parser<char, SecondLifeRegionName, Error = Simple<char>> {
    text::ident()
        .separated_by(just("%20"))
        .collect::<Vec<String>>()
        .map(|components| SecondLifeRegionName(components.join(" ")))
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
#[must_use]
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
#[must_use]
pub fn uuid_parser() -> impl Parser<char, uuid::Uuid, Error = Simple<char>> {
    one_of("0123456789abcdef")
        .repeated()
        .exactly(8)
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
            uuid::Uuid::parse_str(&format!("{}-{}-{}-{}-{}", a, b, c, d, e))
                .map_err(|e| Simple::custom(span.clone(), format!("{:?}", e)))
        })
}

/// parse an agent URL into a SecondLifeAvatarKey
///
/// "secondlife:///app/agent/daf2e68d-6ccc-4592-a049-3306e4820821/about"
///
/// # Errors
///
/// returns an error if the string could not be parsed
#[must_use]
pub fn agent_url_as_avatar_key_parser(
) -> impl Parser<char, SecondLifeAvatarKey, Error = Simple<char>> {
    just("secondlife:///app/agent/")
        .ignore_then(uuid_parser())
        .then_ignore(just("/about").or(just("/inspect")))
        .map(SecondLifeAvatarKey)
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
#[must_use]
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
#[must_use]
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
        /// when buying an object the name of the object
        object_name: Option<String>,
    },
    /// message about a received payment
    ReceivedPaymentMessage {
        /// the sender avatar UUID
        sender_avatar_key: SecondLifeAvatarKey,
        /// the amount received
        amount: SecondLifeLindenAmount,
        /// an optional message
        message: Option<String>,
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
    /// message about successfully shared items
    ItemsSuccessfullyShared,
    /// message about a modified search query
    ModifiedSearchQuery {
        /// the modified query
        query: String,
    },
    /// message about different simulator version
    SimulatorVersion {
        /// the previous region simulator version
        previous_region_simulator_version: String,
        /// the current region simulator version
        current_region_simulator_version: String,
    },
    /// message about a renamed avatar
    RenamedAvatar {
        /// the old name
        old_name: String,
        /// the new name
        new_name: String,
    },
    /// other system message
    OtherSystemMessage {
        /// the raw message
        message: String,
    },
}

/// parse a system message about a saved snapshot
///
/// # Errors
///
/// returns an error if the string could not be parsed
#[must_use]
pub fn snapshot_saved_message_parser(
) -> impl Parser<char, SecondLifeSystemMessage, Error = Simple<char>> {
    just("Snapshot saved: ")
        .ignore_then(any().repeated().collect::<String>().map(PathBuf::from))
        .try_map(|filename, _span: std::ops::Range<usize>| {
            Ok(SecondLifeSystemMessage::SavedSnapshotMessage { filename })
        })
}

/// parse a system message about a saved attachment
///
/// # Errors
///
/// returns an error if the string could not be parsed
#[must_use]
pub fn attachment_saved_message_parser(
) -> impl Parser<char, SecondLifeSystemMessage, Error = Simple<char>> {
    just("Attachment has been saved").try_map(|_, _span: std::ops::Range<usize>| {
        Ok(SecondLifeSystemMessage::AttachmentSavedMessage)
    })
}

/// parse a system message about a sent payment
///
/// # Errors
///
/// returns an error if the string could not be parsed
#[must_use]
pub fn sent_payment_message_parser(
) -> impl Parser<char, SecondLifeSystemMessage, Error = Simple<char>> {
    just("You paid ")
        .ignore_then(agent_url_as_avatar_key_parser())
        .then_ignore(just(" "))
        .then(linden_amount_parser())
        .then(
            just(" for ")
                .ignore_then(take_until(just(".")).map(|(n, _)| Some(n)))
                .or(just(".").map(|_| None)),
        )
        .try_map(
            |((recipient_avatar_key, amount), object_name), _span: std::ops::Range<usize>| {
                Ok(SecondLifeSystemMessage::SentPaymentMessage {
                    recipient_avatar_key,
                    amount,
                    object_name: object_name.map(|n| n.into_iter().collect()),
                })
            },
        )
}

/// parse a system message about a received payment
///
/// # Errors
///
/// returns an error if the string could not be parsed
#[must_use]
pub fn received_payment_message_parser(
) -> impl Parser<char, SecondLifeSystemMessage, Error = Simple<char>> {
    agent_url_as_avatar_key_parser()
        .then_ignore(just(" paid you "))
        .then(linden_amount_parser())
        .then(
            just(": ")
                .ignore_then(any().repeated().collect::<String>())
                .ignore_then(take_until(just(".")).map(|(n, _)| Some(n)))
                .or(just(".").map(|_| None)),
        )
        .try_map(
            |((sender_avatar_key, amount), message), _span: std::ops::Range<usize>| {
                Ok(SecondLifeSystemMessage::ReceivedPaymentMessage {
                    sender_avatar_key,
                    amount,
                    message: message.map(|n| n.into_iter().collect()),
                })
            },
        )
}

/// parse a system message about a completed teleport
///
/// # Errors
///
/// returns an error if the string could not be parsed
#[must_use]
pub fn teleport_completed_message_parser(
) -> impl Parser<char, SecondLifeSystemMessage, Error = Simple<char>> {
    just("Teleport completed from http://maps.secondlife.com/secondlife/")
        .ignore_then(location_parser())
        .try_map(|origin, _span: std::ops::Range<usize>| {
            Ok(SecondLifeSystemMessage::TeleportCompletedMessage { origin })
        })
}

/// parse a system message about a now playing song
///
/// # Errors
///
/// returns an error if the string could not be parsed
#[must_use]
pub fn now_playing_message_parser(
) -> impl Parser<char, SecondLifeSystemMessage, Error = Simple<char>> {
    just("Now playing: ")
        .ignore_then(any().repeated().collect::<String>())
        .try_map(|song_name, _span: std::ops::Range<usize>| {
            Ok(SecondLifeSystemMessage::NowPlayingMessage { song_name })
        })
}

/// parse a system message about a region restart
///
/// # Errors
///
/// returns an error if the string could not be parsed
#[must_use]
pub fn region_restart_message_parser(
) -> impl Parser<char, SecondLifeSystemMessage, Error = Simple<char>> {
    just("The region you are in now is about to restart. If you stay in this region you will be logged out.")
        .try_map(|_, _span: std::ops::Range<usize>| {
            Ok(SecondLifeSystemMessage::RegionRestartMessage)
        })
}

/// parse a system message about an object giving the current avatar an object
///
/// # Errors
///
/// returns an error if the string could not be parsed
#[must_use]
pub fn object_gave_object_message_parser(
) -> impl Parser<char, SecondLifeSystemMessage, Error = Simple<char>> {
    take_until(just(" owned by "))
        .then(agent_url_as_avatar_key_parser())
        .then_ignore(
            whitespace()
                .or_not()
                .ignore_then(just("gave you ").then(just("<nolink>'").or_not())),
        )
        .then(take_until(
            just("</nolink>'")
                .or_not()
                .then(whitespace())
                .then(just("( http://slurl.com/secondlife/")),
        ))
        .then(location_parser())
        .then_ignore(just(" )."))
        .try_map(
            |(
                (((giving_object_name, _), giving_object_owner), (given_object_name, _)),
                giving_object_location,
            ),
             _span: std::ops::Range<usize>| {
                Ok(SecondLifeSystemMessage::ObjectGaveObjectMessage {
                    giving_object_name: giving_object_name.into_iter().collect(),
                    giving_object_owner,
                    given_object_name: given_object_name.into_iter().collect(),
                    giving_object_location,
                })
            },
        )
}

/// parse a system message about an avatar giving the current avatar an object
///
/// # Errors
///
/// returns an error if the string could not be parsed
#[must_use]
pub fn avatar_gave_object_message_parser(
) -> impl Parser<char, SecondLifeSystemMessage, Error = Simple<char>> {
    just("A group member named ")
        .ignore_then(take_until(just(" gave you ")))
        .then(take_until(just(".")))
        .try_map(
            |((giving_avatar_name, _), (given_object_name, _)), _span: std::ops::Range<usize>| {
                Ok(SecondLifeSystemMessage::AvatarGaveObjectMessage {
                    giving_avatar_name: giving_avatar_name.into_iter().collect(),
                    given_object_name: given_object_name.into_iter().collect(),
                })
            },
        )
}

/// parse a system message about items being successfully shared
///
/// # Errors
///
/// returns an error if the string could not be parsed
#[must_use]
pub fn items_successfully_shared_message_parser(
) -> impl Parser<char, SecondLifeSystemMessage, Error = Simple<char>> {
    just("Items successfully shared.").try_map(|_, _span: std::ops::Range<usize>| {
        Ok(SecondLifeSystemMessage::ItemsSuccessfullyShared)
    })
}

/// parse a system message about a modified search query
///
/// # Errors
///
/// returns an error if the string could not be parsed
#[must_use]
pub fn modified_search_query_message_parser(
) -> impl Parser<char, SecondLifeSystemMessage, Error = Simple<char>> {
    just("Your search query was modified and the words that were too short were removed.")
        .ignore_then(whitespace())
        .ignore_then(just("Searched for:"))
        .ignore_then(whitespace())
        .ignore_then(any().repeated().collect::<String>())
        .try_map(|query, _span: std::ops::Range<usize>| {
            Ok(SecondLifeSystemMessage::ModifiedSearchQuery { query })
        })
}

/// parse a system message about a different simulator version
///
/// # Errors
///
/// returns an error if the string could not be parsed
#[must_use]
pub fn simulator_version_message_parser(
) -> impl Parser<char, SecondLifeSystemMessage, Error = Simple<char>> {
    just("The region you have entered is running a different simulator version.")
        .ignore_then(whitespace())
        .ignore_then(just("Current simulator:"))
        .ignore_then(whitespace())
        .ignore_then(take_until(just("\n")).map(|(s, _): (Vec<char>, _)| s.into_iter().collect()))
        .then_ignore(whitespace())
        .then_ignore(just("Previous simulator:"))
        .then_ignore(whitespace())
        .then(any().repeated().collect::<String>())
        .try_map(
            |(current_region_simulator_version, previous_region_simulator_version),
             _span: std::ops::Range<usize>| {
                Ok(SecondLifeSystemMessage::SimulatorVersion {
                    previous_region_simulator_version,
                    current_region_simulator_version,
                })
            },
        )
}

/// parse a system message about a renamed avatar
///
/// # Errors
///
/// returns an error if the string could not be parsed
#[must_use]
pub fn renamed_avatar_message_parser(
) -> impl Parser<char, SecondLifeSystemMessage, Error = Simple<char>> {
    take_until(just(" is now known as"))
        .map(|(s, _)| s.into_iter().collect())
        .then_ignore(whitespace())
        .then(take_until(just(".")).map(|(s, _): (Vec<char>, _)| s.into_iter().collect()))
        .try_map(|(old_name, new_name), _span: std::ops::Range<usize>| {
            Ok(SecondLifeSystemMessage::RenamedAvatar { old_name, new_name })
        })
}

/// parse a Second Life system message
///
/// TODO:
/// You decline...
/// Creating bridge...
/// Bridge created...
/// Script info...
/// Unable to initiate teleport due to RLV restrictions
/// Gave you messages without nolink tags
///
/// # Errors
///
/// returns an error if the string could not be parsed
#[must_use]
pub fn system_message_parser() -> impl Parser<char, SecondLifeSystemMessage, Error = Simple<char>> {
    snapshot_saved_message_parser().or(attachment_saved_message_parser().or(
        sent_payment_message_parser().or(received_payment_message_parser().or(
            teleport_completed_message_parser().or(now_playing_message_parser().or(
                region_restart_message_parser().or(object_gave_object_message_parser().or(
                    items_successfully_shared_message_parser().or(
                        modified_search_query_message_parser().or(
                            avatar_gave_object_message_parser().or(
                                simulator_version_message_parser().or(
                                    renamed_avatar_message_parser().or(any()
                                        .repeated()
                                        .collect::<String>()
                                        .try_map(|s, _span: std::ops::Range<usize>| {
                                            Ok(SecondLifeSystemMessage::OtherSystemMessage {
                                                message: s,
                                            })
                                        })),
                                ),
                            ),
                        ),
                    ),
                )),
            )),
        )),
    ))
}

/// represents a Second Life chat volume
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
    #[must_use]
    pub fn volume_and_message(s: String) -> (SecondLifeChatVolume, String) {
        if let Some(whisper_message) = s.strip_prefix("whispers: ") {
            (SecondLifeChatVolume::Whisper, whisper_message.to_string())
        } else if let Some(shout_message) = s.strip_prefix("shouts: ") {
            (SecondLifeChatVolume::Shout, shout_message.to_string())
        } else {
            (SecondLifeChatVolume::Say, s)
        }
    }
}

/// represents a Second Life area of significance
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SecondLifeArea {
    /// chat range
    ChatRange,
    /// draw distance
    DrawDistance,
    /// region
    Region,
}

/// parse an area of significance
///
/// # Errors
///
/// returns an error if the string could not be parsed
#[must_use]
pub fn area_of_significance_parser() -> impl Parser<char, SecondLifeArea, Error = Simple<char>> {
    just("chat range")
        .to(SecondLifeArea::ChatRange)
        .or(just("draw distance").to(SecondLifeArea::DrawDistance))
        .or(just("the ")
            .or_not()
            .ignore_then(just("region"))
            .to(SecondLifeArea::Region))
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

/// parse a message about an avatar coming online
///
/// # Errors
///
/// returns an error if the parser fails
fn avatar_came_online_message_parser(
) -> impl Parser<char, SecondLifeAvatarMessage, Error = Simple<char>> {
    just("is online.").map(|_| SecondLifeAvatarMessage::CameOnline)
}

/// parse a message about an avatar going offline
///
/// # Errors
///
/// returns an error if the parser fails
fn avatar_went_offline_message_parser(
) -> impl Parser<char, SecondLifeAvatarMessage, Error = Simple<char>> {
    just("is offline.").map(|_| SecondLifeAvatarMessage::WentOffline)
}

/// parse a message about an avatar entering an area of significance
///
/// # Errors
///
/// returns an error if the parser fails
fn avatar_entered_area_message_parser(
) -> impl Parser<char, SecondLifeAvatarMessage, Error = Simple<char>> {
    just("entered ")
        .ignore_then(area_of_significance_parser())
        .then(
            just(" (")
                .ignore_then(distance_parser())
                .then_ignore(just(")"))
                .or_not(),
        )
        .then_ignore(just("."))
        .try_map(|(area, distance), _span: std::ops::Range<usize>| {
            Ok(SecondLifeAvatarMessage::EnteredArea { area, distance })
        })
}

/// parse a message about an avatar leaving an area of significance
///
/// # Errors
///
/// returns an error if the parser fails
fn avatar_left_area_message_parser(
) -> impl Parser<char, SecondLifeAvatarMessage, Error = Simple<char>> {
    just("left ")
        .ignore_then(area_of_significance_parser())
        .then_ignore(just("."))
        .try_map(|area, _span: std::ops::Range<usize>| {
            Ok(SecondLifeAvatarMessage::LeftArea { area })
        })
}

/// parse a Second Life avatar message
///
/// # Errors
///
/// returns an error if the parser fails
fn avatar_message_parser() -> impl Parser<char, SecondLifeAvatarMessage, Error = Simple<char>> {
    avatar_came_online_message_parser().or(avatar_went_offline_message_parser().or(
        avatar_entered_area_message_parser().or(avatar_left_area_message_parser()
            .or(avatar_emote_message_parser().or(avatar_chat_message_parser()))),
    ))
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
            .then_ignore(just(":").then(whitespace()))
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
    /// timestamp of the chat log line, some log lines do not have one because of bugs at the time they were written (e.g. some just have the time formatting string)
    timestamp: Option<time::PrimitiveDateTime>,
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
        .then_ignore(just("]"))
        .try_map(
            |(((((year, month), day), hour), minute), second),
             span: std::ops::Range<usize>| {
                let second = second.unwrap_or("00".to_string());
                let format = time::macros::format_description!(
                    "[year]/[month]/[day] [hour]:[minute]:[second]"
                );
                Ok(Some(
                    time::PrimitiveDateTime::parse(
                        &format!("{}/{}/{} {}:{}:{}", year, month, day, hour, minute, second),
                        format,
                    ).map_err(|e| Simple::custom(span, format!("{:?}", e)))?
                ))
             }
        )
        .or(just("[[year,datetime,slt]/[mthnum,datetime,slt]/[day,datetime,slt] [hour,datetime,slt]:[min,datetime,slt]]").map(|_| None))
        .then_ignore(whitespace())
        .then(chat_log_event_parser())
        .try_map(
            |(timestamp, event),
             _span: std::ops::Range<usize>| {
                Ok(SecondLifeChatLogLine {
                    timestamp,
                    event,
                })
            },
        )
}

/// determine avatar log dir from avatar name
fn avatar_log_dir(avatar_name: &str) -> Result<PathBuf, crate::Error> {
    let avatar_dir_name = avatar_name.replace(' ', "_").to_lowercase();
    tracing::debug!("Avatar dir name: {}", avatar_dir_name);

    let Some(home_dir) = dirs2::home_dir() else {
        tracing::error!("Could not determine current user home directory");
        return Err(crate::Error::HomeDirError);
    };

    Ok(home_dir.join(".firestorm/").join(avatar_dir_name))
}

/// parse a chat line as a welcome greeting and return the names of the greeted people
///
/// # Errors
///
/// returns an error if the parser fails
fn welcome_greeting_parser() -> impl Parser<char, Vec<String>, Error = Simple<char>> {
    just("hi")
        .or(just("hello"))
        .or(just("hallo"))
        .or(just("ahoy"))
        .or(just("wb"))
        .or(just("welcome back"))
        .ignore_then(whitespace())
        .ignore_then(
            take_until(
                just(",")
                    .or(just("and"))
                    .or(just("und"))
                    .or(just("\n").or(end().map(|_| "")))
                    .rewind(),
            )
            .separated_by(just(",").or(just("and")).or(just("und")).or(just("\n"))),
        )
        .try_map(|s, _span: std::ops::Range<usize>| {
            Ok(s.into_iter()
                .map(|(s, _)| s.into_iter().collect::<String>().trim().to_string())
                .collect())
        })
}

/// The main behaviour of the binary should go here
#[instrument]
async fn do_stuff() -> Result<(), crate::Error> {
    let options = <Options as clap::Parser>::parse();
    tracing::debug!("{:#?}", options);

    let avatar_dir = avatar_log_dir(&options.avatar_name)?;

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

    let mut last_line: Option<String> = None;

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);

    let join_handle = tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Err(e) = tx.send(line).await {
                tracing::error!("Error sending line: {:?}", e);
            }
        }
    });

    let (tx2, mut rx2) = tokio::sync::mpsc::channel(16);

    let join_handle2 = tokio::spawn(async move {
        loop {
            match tokio::time::timeout(std::time::Duration::from_millis(1), rx.recv()).await {
                Err(tokio::time::error::Elapsed { .. }) => {
                    if let Some(ref ll) = last_line {
                        if let Err(e) = tx2.send(ll.clone()).await {
                            tracing::error!("Error sending line (tx2): {:?}", e);
                        }
                        last_line = None;
                    }
                }
                Ok(Some(line)) => {
                    last_line = if let Some(ref ll) = last_line {
                        if line.line().starts_with(' ') || line.line() == "" {
                            Some(format!("{}\n{}", ll, line.line()))
                        } else {
                            if let Err(e) = tx2.send(ll.clone()).await {
                                tracing::error!("Error sending line (tx2): {:?}", e);
                            }
                            Some(line.line().to_string())
                        }
                    } else {
                        Some(line.line().to_string())
                    };
                }
                _ => {
                    break;
                }
            }
        }
    });

    let mut notify_handles: BTreeMap<String, notify_rust::NotificationHandle> = BTreeMap::new();
    let mut last_seen_in_chat_range: BTreeMap<String, time::PrimitiveDateTime> = BTreeMap::new();

    while let Some(line) = rx2.recv().await {
        println!("parsing line:\n{}", line);
        let parsed_line = sl_chat_log_line_parser().parse(line.clone());
        println!("parse result:\n{:#?}", parsed_line);

        if let Ok(SecondLifeChatLogLine {
            timestamp,
            event:
                SecondLifeChatLogEvent::AvatarLine {
                    ref name,
                    message:
                        SecondLifeAvatarMessage::EnteredArea {
                            area: SecondLifeArea::ChatRange,
                            distance: _,
                        },
                },
        }) = parsed_line
        {
            let (last_seen_description, last_seen_age) = if let Some(last_seen_timestamp) =
                last_seen_in_chat_range.get(&name.to_lowercase())
            {
                if let Some(timestamp) = timestamp {
                    let last_seen_age = timestamp - *last_seen_timestamp;
                    if let Ok(std_last_seen_age) = last_seen_age.try_into() {
                        (
                            format!(
                                "Last seen {} ago ({})",
                                <humantime::Duration as From<std::time::Duration>>::from(
                                    std_last_seen_age
                                ),
                                last_seen_timestamp
                            ),
                            Some(last_seen_age),
                        )
                    } else {
                        (
                            format!(
                                "Could not convert last seen age to humantime: {}",
                                last_seen_age
                            ),
                            Some(last_seen_age),
                        )
                    }
                } else {
                    (
                        "Unable to determine timestamp for current message".to_string(),
                        None,
                    )
                }
            } else {
                ("Not seen recently".to_string(), None)
            };
            if last_seen_age.is_none()
                || last_seen_age
                    .is_some_and(|last_seen_age| last_seen_age > std::time::Duration::from_secs(5))
            {
                match notify_rust::Notification::new()
                    .appname("sl-hello-goodbye")
                    .summary("New person entered chat range")
                    .body(&format!(
                        "{} entered the chat range\n{}",
                        name, last_seen_description
                    ))
                    .hint(notify_rust::Hint::Resident(true))
                    .timeout(notify_rust::Timeout::Never)
                    .show()
                {
                    Ok(notify_handle) => {
                        notify_handles.insert(name.to_string().to_lowercase(), notify_handle);
                    }
                    Err(e) => {
                        tracing::error!("Error sending notification: {:?}", e);
                    }
                }
            }
            if let Some(timestamp) = timestamp {
                last_seen_in_chat_range.insert(name.to_lowercase(), timestamp);
            }
        }

        if let Ok(SecondLifeChatLogLine {
            timestamp,
            event:
                SecondLifeChatLogEvent::AvatarLine {
                    ref name,
                    message:
                        SecondLifeAvatarMessage::LeftArea {
                            area: SecondLifeArea::ChatRange,
                        },
                },
        }) = parsed_line
        {
            if let Some(timestamp) = timestamp {
                last_seen_in_chat_range.insert(name.to_lowercase(), timestamp);
            }
            let name = name.to_lowercase();
            let mut to_remove = Vec::new();
            for n in notify_handles.keys() {
                if *n == name {
                    to_remove.push(name.to_string());
                }
            }
            for name in to_remove {
                if let Some(notify_handle) = notify_handles.remove(&name) {
                    notify_handle.close();
                }
            }
        }

        // TODO:
        // leave announcements and left chat range

        if let Ok(SecondLifeChatLogLine {
            timestamp,
            event:
                SecondLifeChatLogEvent::AvatarLine {
                    ref name,
                    message:
                        SecondLifeAvatarMessage::Chat {
                            ref message,
                            volume,
                        },
                },
        }) = parsed_line
        {
            if *name == options.avatar_name {
                if let Ok(greeted) = welcome_greeting_parser().parse(message.to_lowercase()) {
                    tracing::debug!("Found welcoming greeting greeting\n{:#?}", greeted);
                    for greeted in greeted {
                        let greeted = greeted.to_lowercase();
                        let mut to_remove = Vec::new();
                        for name in notify_handles.keys() {
                            if name.contains(&greeted) {
                                to_remove.push(name.to_string());
                            }
                        }
                        for name in to_remove {
                            if let Some(notify_handle) = notify_handles.remove(&name) {
                                notify_handle.close();
                            }
                        }
                    }
                }
            } else if let Some(timestamp) = timestamp {
                if volume <= SecondLifeChatVolume::Say {
                    last_seen_in_chat_range.insert(name.to_lowercase(), timestamp);
                }
            }
        }

        if let Ok(SecondLifeChatLogLine {
            timestamp: Some(timestamp),
            event:
                SecondLifeChatLogEvent::AvatarLine {
                    name,
                    message: SecondLifeAvatarMessage::Emote { message: _, volume },
                },
        }) = parsed_line
        {
            if volume <= SecondLifeChatVolume::Say {
                last_seen_in_chat_range.insert(name.to_lowercase(), timestamp);
            }
        }
    }

    join_handle.await?;
    join_handle2.await?;

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
    use std::io::{BufRead, BufReader};

    use super::*;
    use pretty_assertions::assert_eq;

    /// used to deserialize the required options from the environment
    #[derive(Debug, serde::Deserialize)]
    struct EnvOptions {
        #[serde(
            deserialize_with = "serde_aux::field_attributes::deserialize_vec_from_string_or_vec"
        )]
        test_avatar_names: Vec<String>,
    }

    /// Error enum for the application
    #[derive(thiserror::Error, Debug)]
    pub enum TestError {
        /// error loading environment
        #[error("error loading environment: {0}")]
        EnvError(#[from] envy::Error),
        /// error loading .env file
        #[error("error loading .env file: {0}")]
        DotEnvError(#[from] dotenvy::Error),
        /// error opening chat log file
        #[error("error opening chat log file {0}: {1}")]
        OpenChatLogFileError(std::path::PathBuf, std::io::Error),
        /// error reading chat log line from file
        #[error("error reading chat log line from file: {0}")]
        ChatLogLineReadError(std::io::Error),
        /// application error
        #[error(transparent)]
        AppError(#[from] crate::Error),
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn test_log_line_parser() -> Result<(), TestError> {
        dotenvy::dotenv()?;
        let env_options = envy::from_env::<EnvOptions>()?;
        for avatar_name in env_options.test_avatar_names {
            let avatar_dir = avatar_log_dir(&avatar_name)?;
            let local_chat_log_file = avatar_dir.join("chat.txt");
            let file = std::fs::File::open(&local_chat_log_file)
                .map_err(|e| TestError::OpenChatLogFileError(local_chat_log_file.clone(), e))?;
            let file = BufReader::new(file);
            let mut last_line: Option<String> = None;
            for line in file.lines() {
                let line = line.map_err(TestError::ChatLogLineReadError)?;
                if line.starts_with(" ") || line == "" {
                    if let Some(ll) = last_line {
                        last_line = Some(format!("{}\n{}", ll, line));
                        continue;
                    }
                }
                if let Some(ref ll) = last_line {
                    match sl_chat_log_line_parser().parse(ll.clone()) {
                        Err(e) => {
                            tracing::error!("failed to parse line\n{}", ll);
                            for err in e {
                                tracing::error!("{}", err);
                            }
                            panic!("Failed to parse a line");
                        }
                        Ok(parsed_line) => {
                            if let SecondLifeChatLogLine {
                                timestamp: _,
                                event:
                                    SecondLifeChatLogEvent::SystemMessage {
                                        message:
                                            SecondLifeSystemMessage::OtherSystemMessage { ref message },
                                    },
                            } = parsed_line
                            {
                                tracing::info!("parsed line\n{}\n{:?}", ll, parsed_line);
                                if message.contains("owned by") && message.contains("gave you") {
                                    if let Err(e) = object_gave_object_message_parser()
                                        .parse(message.to_string())
                                    {
                                        for e in e {
                                            tracing::debug!("Attempt to parse as object gave object line returned error:\n{}\n{:#?}", e, e);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                last_line = Some(line);
            }
        }
        panic!();
        //Ok(())
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn test_welcome_greeting_parser_one_avatar() -> Result<(), TestError> {
        match welcome_greeting_parser().parse("hello john") {
            Ok(parsed) => {
                assert_eq!(parsed, ["john"]);
            }
            Err(e) => {
                for err in e {
                    tracing::error!("{}", err);
                }
                panic!("Failed to parse a line");
            }
        }
        Ok(())
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn test_welcome_greeting_parser_two_avatars() -> Result<(), TestError> {
        match welcome_greeting_parser().parse("hello john and paul") {
            Ok(parsed) => {
                assert_eq!(parsed, ["john", "paul"]);
            }
            Err(e) => {
                for err in e {
                    tracing::error!("{}", err);
                }
                panic!("Failed to parse a line");
            }
        }
        Ok(())
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn test_welcome_greeting_parser_three_avatars() -> Result<(), TestError> {
        match welcome_greeting_parser().parse("hello john, paul and mary") {
            Ok(parsed) => {
                assert_eq!(parsed, ["john", "paul", "mary"]);
            }
            Err(e) => {
                for err in e {
                    tracing::error!("{}", err);
                }
                panic!("Failed to parse a line");
            }
        }
        Ok(())
    }
}
