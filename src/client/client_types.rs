//
// Copyright (C) 2024 Automated Design Corp.. All Rights Reserved.
// Created Date: 2024-04-09 08:17:55
// -----
// Last Modified: 2024-04-09 14:31:08
// -----
// 
//


//! Types used through the client module. 


use std::fmt;
use zerocopy_derive::{AsBytes, FromBytes, FromZeroes};

use super::ads_data::AdsDataTypeId;

/// Stores the event details from an on Data change notification from
/// the ADS router. This structure is passed via a channel from the 
/// ADS Router thread context into the thread context of the AdsClient.
pub struct DataChangeEventInfo {
    /// Id of the symbol about which the notification was received.
    pub id : u32,
    /// The bytes received from the ADS router    
    pub data : Vec<u8>
}



/// Properties of a symbol that has been successfully registered for 
/// on-change notification.
#[derive(Debug, Clone)]
pub struct RegisteredSymbol {
    pub handle : u32,
    pub notification_handle : u32,
    pub name : String,
    pub data_type_id : AdsDataTypeId
}



/// A public type for fixed-length strings in the PLC. Represents
/// T_MaxString, which is an array of 255 character bytes.
#[repr(C)]
#[derive(FromBytes, FromZeroes, AsBytes, Debug, Clone, Copy)]
pub struct MaxString([u8;256]);


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
            return String::from_utf8(self.0[..end_index].to_vec());
        }
        else {
            return Ok(String::from(""));
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