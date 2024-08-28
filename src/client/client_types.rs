// Copyright (C) 2024 Automated Design Corp.. All Rights Reserved.
use std::fmt;
use std::time::{Duration, Instant, SystemTime};

use mechutil::variant::VariantValue;
use serde::{de::Visitor, Deserialize, Deserializer, Serialize, Serializer};
use zerocopy_derive::{AsBytes, FromBytes, FromZeroes};

use super::ads_data::AdsDataTypeId;

lazy_static! {
    /// A static reference Instant, initialized at application start.
    static ref REFERENCE_INSTANT: Instant = Instant::now();
}

/// Enumerates the type of event that is being transmitted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventInfoType {
    Invalid = 0,
    /// The value of a symbol in the target device has changed.
    /// When a symbol is initially registered for on data change notification, an initial notification of its
    /// current value will be broadcast.
    DataChange = 1,
    /// The state of the target device has changed. This is generally a PLC, and indicates whether
    /// the PLC is in RUN or STOP. When a target is connected, and= initial notification of its
    /// current state will be broadcast.
    AdsState = 2,
    /// The state of the ADS Router has changed. The ADS Router is the pipeline to the targets.
    /// Note that Router callbacks will only be issued when the Router state changes, and there
    /// will be no initial callback when the connection is made, unlike other notification types.
    RouterState = 3,
    /// The state of the symbol table in the target device has changed.
    /// This requires unregistering and re-registering all handles and notifications.
    SymbolTableChange = 10,
}

/// Stores the event details from a notification from
/// the ADS router. This structure is passed via a channel from the
/// ADS Router thread context into the thread context of the AdsClient.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterNotificationEventInfo {
    /// The type of event that is being transmitted.
    pub event_type: EventInfoType,
    /// Id of the symbol about which the notification was received.
    pub id: u32,
    /// The bytes received from the ADS router    
    pub data: Vec<u8>,
}

/// Serialize `Instant` as a Unix timestamp in milliseconds
fn serialize_instant<S>(instant: &Instant, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let duration_since_epoch = instant.duration_since(*REFERENCE_INSTANT);
    let timestamp_in_ms = duration_since_epoch.as_millis();
    serializer.serialize_u64(timestamp_in_ms as u64)
}

/// Deserialize `Instant` from a Unix timestamp in milliseconds
fn deserialize_instant<'de, D>(deserializer: D) -> Result<Instant, D::Error>
where
    D: Deserializer<'de>,
{
    struct InstantVisitor;

    impl<'de> Visitor<'de> for InstantVisitor {
        type Value = Instant;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a UNIX timestamp in milliseconds")
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            let since_epoch = Duration::from_millis(value);
            SystemTime::UNIX_EPOCH
                .checked_add(since_epoch)
                .and_then(|time| time.duration_since(SystemTime::now()).ok())
                .map(|duration| Instant::now() + duration)
                .ok_or_else(|| E::custom("invalid timestamp"))
        }
    }

    deserializer.deserialize_u64(InstantVisitor)
}

/// Notification data for a registered symbol data change event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdsClientNotification {
    /// The type of event that is being transmitted.
    pub event_type: EventInfoType,
    /// Monotonic tick representing when the notification was received.
    #[serde(
        serialize_with = "serialize_instant",
        deserialize_with = "deserialize_instant"
    )]
    pub timestamp: Instant,
    /// Name of the symbol or tag for which this value was received
    /// if event_info is DataChange. Not used for other types.
    pub name: String,
    /// The value received
    pub value: VariantValue,
}

impl Default for AdsClientNotification {
    fn default() -> Self {
        Self {
            event_type: EventInfoType::Invalid,
            timestamp: Instant::now(),
            name: String::new(),
            value: VariantValue::Null,
        }
    }
}

impl AdsClientNotification {
    pub fn new_datachange(name: &str, val: &VariantValue) -> Self {
        Self {
            event_type: EventInfoType::DataChange,
            timestamp: Instant::now(),
            name: name.to_string(),
            value: val.clone(),
        }
    }

    pub fn new_ads_state(state: i16) -> Self {
        Self {
            event_type: EventInfoType::AdsState,
            timestamp: Instant::now(),
            name: String::new(),
            value: VariantValue::from(state),
        }
    }

    pub fn new_router_state(state: i16) -> Self {
        Self {
            event_type: EventInfoType::RouterState,
            timestamp: Instant::now(),
            name: String::new(),
            value: VariantValue::from(state),
        }
    }

    /// Creates an event that represents a change to the symbol table.
    pub fn new_symbol_table() -> Self {
        Self {
            event_type: EventInfoType::SymbolTableChange,
            timestamp: Instant::now(),
            name: String::new(),
            value: VariantValue::Null,
        }
    }
}

/// Properties of a symbol that has been successfully registered for
/// on-change notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisteredSymbol {
    pub handle: u32,
    pub notification_handle: u32,
    pub name: String,
    pub data_type_id: AdsDataTypeId,
    pub options: serde_json::Map<String, serde_json::Value>,
}

/// A public type for fixed-length strings in the PLC. Represents
/// T_MaxString, which is an array of 255 character bytes.
/// Does not work for strings in the PLC that are defined with a
/// shorter length.
#[repr(C)]
#[derive(FromBytes, FromZeroes, AsBytes, Debug, Clone, Copy)]
pub struct MaxString([u8; 256]);

/// Implement a default value for MaxString, which is an array of character bytes.
///  Rust automatically implements Default for arrays with up to 32 elements.
/// For arrays larger than that, you must manually implement Default.
impl Default for MaxString {
    fn default() -> Self {
        MaxString([0; 256])
    }
}

impl fmt::Display for MaxString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Use the to_string method you defined to get a String representation
        match self.to_string() {
            Ok(str) => write!(f, "{}", str),
            Err(_) => write!(f, "<Invalid UTF-8 data>"),
        }
    }
}

impl MaxString {
    // Method to convert MaxString to a Rust String
    pub fn to_string(&self) -> Result<String, std::string::FromUtf8Error> {
        if let Some(end_index) = self.0.iter().position(|&c| c == 0x00) {
            String::from_utf8(self.0[..end_index].to_vec())
        } else {
            Ok(String::new())
        }
    }

    // Method to create MaxString from a Rust String
    pub fn from_string(s: &str) -> Self {
        let mut array = [0u8; 256];
        let bytes = &s.as_bytes()[..s.len().min(255)];
        array[..bytes.len()].copy_from_slice(bytes);
        MaxString(array)
    }
}

/// Defines the ADS state of a target device. Generally, this means
/// whether or not the PLC is in RUN or STOPPED.
/// ```ignore
/// match AdsState::from(notification.value) {
///     AdsState::Started => log::info!("Target device is RUNNING."),
///     AdsState::Stopped => log::info!("Target device is STOPPED.")
///     AdsState::Unknown => log::info!("Target device is in an unknown state.")
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AdsState {
    /// No information is known about the target device
    Unknown = -1,
    /// The target device is in Run Mode
    Running = 5,
    /// The target device is stopped.
    Stopped = 6,
}

impl From<i16> for AdsState {
    fn from(value: i16) -> Self {
        match value {
            5 => AdsState::Running,
            6 => AdsState::Stopped,
            _ => AdsState::Unknown,
        }
    }
}

impl From<VariantValue> for AdsState {
    fn from(value: VariantValue) -> Self {
        match value {
            VariantValue::Int32(v) => match v {
                5 => AdsState::Running,
                6 => AdsState::Stopped,
                _ => AdsState::Unknown,
            },
            VariantValue::Int16(v) => match v {
                5 => AdsState::Running,
                6 => AdsState::Stopped,
                _ => AdsState::Unknown,
            },
            VariantValue::Byte(v) => match v as i16 {
                5 => AdsState::Running,
                6 => AdsState::Stopped,
                _ => AdsState::Unknown,
            },
            VariantValue::SByte(v) => match v as i16 {
                5 => AdsState::Running,
                6 => AdsState::Stopped,
                _ => AdsState::Unknown,
            },
            // Potentially include other conversions if reasonable
            _ => AdsState::Unknown, // Fallback for all other types
        }
    }
}

/// Defines the state of the ADS Router.
/// Use this enumeration to easily convert notifications from the client.
///
/// Note that Router callbacks will only be issued when the Router state changes, and there
/// will be no initial callback when the connection is made, unlike other notification types.
///
/// ```ignore
/// match RouterState::from(notification.value) {
///     RouterState::Started => log::info!("Router is RUNNING"),
///     RouterState::Stopped => log::info!("Router is STOPPED"),
///     RouterState::Removed => log::info!("Router has been removed!"),
///     RouterState::Unknown => log::info!("Router is in an unknown state."),
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RouterState {
    /// No information is known about the target device
    Unknown = -1,
    /// The router stopped.
    Stopped = 0,
    /// The router started.
    Started = 1,
    /// The router has been removed.
    Removed = 2,
}

impl From<i16> for RouterState {
    fn from(value: i16) -> Self {
        match value {
            1 => RouterState::Started,
            0 => RouterState::Stopped,
            2 => RouterState::Removed,
            _ => RouterState::Unknown,
        }
    }
}

impl From<VariantValue> for RouterState {
    fn from(value: VariantValue) -> Self {
        match value {
            VariantValue::Int32(v) => match v {
                1 => RouterState::Started,
                0 => RouterState::Stopped,
                2 => RouterState::Removed,
                _ => RouterState::Unknown,
            },
            VariantValue::Int16(v) => match v {
                1 => RouterState::Started,
                0 => RouterState::Stopped,
                2 => RouterState::Removed,
                _ => RouterState::Unknown,
            },
            VariantValue::Byte(v) => match v as i16 {
                1 => RouterState::Started,
                0 => RouterState::Stopped,
                2 => RouterState::Removed,
                _ => RouterState::Unknown,
            },
            VariantValue::SByte(v) => match v as i16 {
                1 => RouterState::Started,
                0 => RouterState::Stopped,
                2 => RouterState::Removed,
                _ => RouterState::Unknown,
            },
            // Potentially include other conversions if reasonable
            _ => RouterState::Unknown, // Fallback for all other types
        }
    }
}
