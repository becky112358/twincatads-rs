// (C) Copyright 2022 Automated Design Corp. All Rights Reserved.
// Author: Thomas C. Bitsky Jr. (support@automateddesign.com)
//

//! Loads symbols from the remote controller dynamically, making it
//! easier for a user program to automatically add groups/domains of
//! symbols instead of hammering out each individual symbol.

use std::{collections::BTreeMap, collections::HashMap};

use zerocopy::FromBytes;

use serde::Deserialize;
use std::os::raw::c_void; // c_void, etc

use anyhow::anyhow;

// Pull in imports from the top crate.
use crate::AdsSymbolEntry;
use crate::AdsSymbolUploadInfo2;
use crate::AdsSyncReadReqEx2;
use crate::ADSIGRP_SYM_DT_UPLOAD;
use crate::ADSIGRP_SYM_UPLOAD;
use crate::ADSIGRP_SYM_UPLOADINFO2;
use crate::{AdsDatatypeArrayInfo, AdsDatatypeEntry, AmsAddr, ADSERR_NOERR};

/// Stores information about a symbol parsed out from the
/// controller.
#[derive(Debug, Deserialize, Clone)]
pub struct AdsSymbolInfo {
    /// Fully-qualified name of the symbol.
    pub name: String,
    /// Index Group of symbol: input, output, etc.
    pub group_index: u32,
    /// Index Offset of symbol.
    pub index_offset: u32,
    /// Size of symbol, in bytes. 0 = bit.
    pub size: u32,

    /// ADS Data Type ID of the symbol
    pub type_id: u32,
    /// ADS Data Type of symbol.
    pub type_name: String,
    /// Comment, if there is one.
    pub comment: String,
    /// If true, this symbol is a structure
    pub is_structure: bool,
    /// If ture, this symbol is an array
    pub is_array: bool,

    /// Dimensions of array, if it is one.
    pub array_dimensions: Vec<usize>,
}

impl AdsSymbolInfo {
    /// Constructor
    pub fn new() -> AdsSymbolInfo {
        AdsSymbolInfo {
            group_index: 0,
            index_offset: 0,
            size: 0,
            name: String::new(),
            type_id: 0,
            type_name: String::new(),
            comment: String::new(),
            is_structure: false,
            is_array: false,
            array_dimensions: Vec::new(),
        }
    }
}

/// Data type information uploaded from the PLC in a more Rust- and user-friendly format.
#[derive(Clone)]
pub struct AdsDataTypeInfo {
    pub name: String,
    pub comment: String,
    pub entry_length: u32,
    pub version: u32,
    pub size: u32,
    pub offset: u32,
    pub data_type: u32,
    pub flags: u32,
    pub num_array_dimensions: u16,
    pub num_fields: u16,
    pub array_data_size: usize,
    pub array_dimensions: Vec<usize>,

    /// Fields of this data type if it is a structure.
    pub fields: Vec<AdsSymbolInfo>,
}

// fn ads_data_type_info_from_type_id_code(
//     type_id: u32,
//     symbol_info : &AdsSymbolInfo
// ) -> Result<AdsDataTypeInfo, anyhow::Error> {

//     if let Ok(dt) = AdsDataTypeId::try_from(value) {

//         match dt {
//             AdsDataTypeId::Void => {
//                 return Err("Can't convert from VOID");
//             },
//             AdsDataTypeId::Int8=> {

//             },
//             AdsDataTypeId::UInt8=> {

//             },
//             AdsDataTypeId::Int16=> {

//             },
//             AdsDataTypeId::UInt16=> {

//             },
//             AdsDataTypeId::Int32=> {

//             },
//             AdsDataTypeId::UInt32=> {

//             },
//             AdsDataTypeId::Int64=> {

//             },
//             AdsDataTypeId::UInt64=> {

//             },
//             AdsDataTypeId::Real32=> {

//             },
//             AdsDataTypeId::Real64=> {

//             },
//             AdsDataTypeId::String=> {
//                 return Ok(AdsDataTypeInfo {
//                     name: symbol_info.name.clone(),
//                     comment: String::new(),
//                     entry_length: 0,
//                     version: 0,
//                     size: symbol_info.size,
//                     offset: symbol_info.index_offset,
//                     data_type: symbol_info.type_id,
//                     flags: 0,
//                     num_array_dimensions: 0,
//                     num_fields: 0,
//                     array_data_size: 0,
//                     array_dimensions: Vec::new(),
//                     fields: Vec::new(),
//                 });
//             },
//             AdsDataTypeId::WString=> {

//             },
//             AdsDataTypeId::Real80=> {

//             },
//             AdsDataTypeId::Bit=> {

//             },
//             AdsDataTypeId::BigType=> {

//             },
//             AdsDataTypeId::MaxTypes=> {

//             },
//             _ => Err("Not a P.O.D type"),
//         }
//     }
//     else {
//         return Err("Not a P.O.D type");
//     }

// }

impl AdsDataTypeInfo {
    /// Default constructor. Creates a blank, useless item that serves only
    /// as a placeholder.
    pub fn new() -> Self {
        return Self {
            name: String::new(),
            comment: String::new(),
            entry_length: 0,
            version: 0,
            size: 0,
            offset: 0,
            data_type: 0,
            flags: 0,
            num_array_dimensions: 0,
            num_fields: 0,
            array_data_size: 0,
            array_dimensions: Vec::new(),
            fields: Vec::new(),
        };
    }

    /// Returns the number of child items this data type contains, be they
    /// from fields (meaning this is a structure) or array indices.
    pub fn sub_item_count(&self) -> usize {
        if self.num_fields > 0 {
            return self.num_fields as usize;
        } else if self.num_array_dimensions > 0 {
            todo!()
            // let mut cnt = 1;
            // PAdsDatatypeArrayInfo pAI = PADSDATATYPEARRAYINFO(pEntry);
            // for (USHORT i = 0; i < pEntry->arrayDim; i++)
            //     cnt *= pAI[i].elements;
        } else {
            return 0;
        }
    }
}

/// A collection of the symbols and data type information uploaded from
/// the target. Used by the client to properly serialize and deserialize data.
#[derive(Clone)]
pub struct AdsSymbolCollection {
    /// A collection of the symbols uploaded from the controller.
    /// A BTreeMap naturally keeps the symbol keys in alphabetical order.
    pub symbols: BTreeMap<String, AdsSymbolInfo>,
    /// A collection of the data types uploaded from the controller.
    pub data_types: BTreeMap<String, AdsDataTypeInfo>,
}

impl AdsSymbolCollection {
    /// Returns the fundamental type of a symbol. If a symbol is plain old data, the returned type
    /// will match what would be returned by simply using the .get method on the data types member.
    /// However, if the symbol is an array or reference, then the fundamental type is the value
    /// listed after the 'OF' word. In other words, if the symbol is an ARRAY [..] OF REAL, then the
    /// fundamental type is REAL, and that is the type info that will be returned.
    ///
    /// Returns NONE if not found.
    pub fn get_fundamental_type_info(
        &self,
        symbol_info: &AdsSymbolInfo,
    ) -> Option<AdsDataTypeInfo> {
        if symbol_info.type_name.contains("OF") {
            if let Some(start) = symbol_info.type_name.find("OF") {
                let type_name = symbol_info.type_name[start + 3..].to_string();
                return self.data_types.get(&type_name).clone().cloned();
            } else {
                // Should not reach here.
                return None;
            }
        } else {
            if let Some(ret) = self.data_types.get(&symbol_info.type_name).clone().cloned() {
                return Some(ret);
            } else {
                // We didn't upload this type, but it obviously exists. Return its generic information that
                // should be useful enough for most situations.
                return Some(AdsDataTypeInfo {
                    name: symbol_info.name.clone(),
                    comment: format!("GENERIC {}", &symbol_info.type_name),
                    entry_length: 0,
                    version: 0,
                    size: symbol_info.size,
                    offset: symbol_info.index_offset,
                    data_type: symbol_info.type_id,
                    flags: 0,
                    num_array_dimensions: 0,
                    num_fields: 0,
                    array_data_size: 0,
                    array_dimensions: Vec::new(),
                    fields: Vec::new(),
                });
            }

            // if symbol_info.type_name.contains("STRING") || symbol_info.type_name.contains("T_MaxString") {
            //     return Some(AdsDataTypeInfo {
            //         name: symbol_info.name.clone(),
            //         comment: String::new(),
            //         entry_length: 0,
            //         version: 0,
            //         size: symbol_info.size,
            //         offset: symbol_info.index_offset,
            //         data_type: symbol_info.type_id,
            //         flags: 0,
            //         num_array_dimensions: 0,
            //         num_fields: 0,
            //         array_data_size: 0,
            //         array_dimensions: Vec::new(),
            //         fields: Vec::new(),
            //     });
            // }
            // else {
            //     //log::debug!("\nFundamental type: {}\n\t{:?}\n", symbol_info.type_name, symbol_info);

            //     // for item in &self.data_types {
            //     //     log::debug!("Type: {}", item.0);
            //     // }

            //     if let Some(ret) = self.data_types.get(&symbol_info.type_name).clone().cloned() {
            //         return Some(ret);
            //     }
            //     else {
            //         // We didn't upload this type, but it obviously exists. Return its generic information.
            //         return Some(AdsDataTypeInfo {
            //             name: symbol_info.name.clone(),
            //             comment: format!("GENERIC {}", &symbol_info.type_name),
            //             entry_length: 0,
            //             version: 0,
            //             size: symbol_info.size,
            //             offset: symbol_info.index_offset,
            //             data_type: symbol_info.type_id,
            //             flags: 0,
            //             num_array_dimensions: symbol_info.array_dimensions.len() as u16,
            //             num_fields: 0,
            //             array_data_size: 0,
            //             array_dimensions: symbol_info.array_dimensions.clone(),
            //             fields: Vec::new(),
            //         });
            //     }
            // }
        }
    }

    /// Get the symbol info for a fully-qualified tag name, included if this is a field of
    /// a structure, in which case some extra work has to be done to extract the info from
    /// the uploaded symbols.
    ///
    ///

    /// Get the symbol info for a fully-qualified tag name. This also handles cases where
    /// the symbol is a field of a structure (including nested structures).
    pub fn get_symbol_info(&self, symbol_name: &str) -> Result<AdsSymbolInfo, anyhow::Error> {
        // Extracts the portion of the symbol name up to the second period, if present.
        let processed_symbol_name = symbol_name.splitn(3, '.').collect::<Vec<&str>>();
        let valid_symbol_name = match processed_symbol_name.as_slice() {
            [first, second, ..] => format!("{}.{}", first, second), // Join the first two segments
            [single] => single.to_string(),                         // Only one segment, use as is
            _ => return Err(anyhow!("Invalid symbol name format")),
        };

        if self.symbols.contains_key(&valid_symbol_name) {

            let symbol = &self.symbols[&valid_symbol_name];
            let tokens = symbol_name.split(".").collect::<Vec<&str>>();

            // Basic, non-field and non-nested cases
            if !symbol.is_structure && tokens.len() < 3 {
                return Ok(symbol.clone());
            } else {
                // Field request - potentially nested
                let remaining_symbol_name = tokens[2..].join(".");

                // Check if the current segment refers to a structure
                if symbol.is_structure {

                    // Or a more accurate structure check
                    // Recursive call for the nested field lookup
                    let nested_field_result = self.get_symbol_info(&remaining_symbol_name);

                    match nested_field_result {
                        Ok(field) => return Ok(field),
                        Err(error) => return Err(error),
                    }
                } else {

                    // this was a request for a specific field
                    // Loop through the fields until we find the specific one we need.

                    let mut tmp_symbol = symbol.clone();
                    for i in 2.. tokens.len() {
                        
                        let field_name = tokens[i];
                        if let Some(dt) = self.get_fundamental_type_info(&tmp_symbol) {
                            for field in dt.fields {
                                if field.name == field_name {
                                    tmp_symbol = field.clone();                                    
                                }
                            }

                        } else {
                            return Err(anyhow!(
                                "Failed to locate information for structure field {}",
                                field_name
                            ));
                        }
    
                    }

                    return Ok(tmp_symbol);
                }
            }
        } else {
            return Err(anyhow!(
                "Cannot locate any information for symbol {}",
                symbol_name
            ));
        }
    }

    // pub fn get_symbol_info(&self, symbol_name : &str) -> Result<AdsSymbolInfo, anyhow::Error> {

    //     // Extracts the portion of the symbol name up to the second period, if present.
    //     let processed_symbol_name = symbol_name.splitn(3, '.').collect::<Vec<&str>>();
    //     let valid_symbol_name = match processed_symbol_name.as_slice() {
    //         [first, second, ..] => format!("{}.{}", first, second), // Join the first two segments
    //         [single] => single.to_string(),  // Only one segment, use as is
    //         _ => return Err(anyhow!("Invalid symbol name format")),
    //     };

    //     if self.symbols.contains_key(&valid_symbol_name) {

    //         let symbol = &self.symbols[&valid_symbol_name];
    //         let tokens = symbol_name.split(".").collect::<Vec<&str>>();

    //         let symbol = &self.symbols[&valid_symbol_name];
    //         let tokens = symbol_name.split(".").collect::<Vec<&str>>();

    //         // NOT: symbol.is_structure is not always correct on whether a type is a
    //         // structure or not. Using tokens.len() is more reliable.
    //         if !symbol.is_structure && tokens.len() < 3 {
    //             // The request was not for a field.
    //             return Ok(symbol.clone());
    //         }
    //         else {

    //             // this was a request for a specific field

    //             let field_name = tokens[2];
    //             if let Some(dt)  = self.get_fundamental_type_info(&symbol) {
    //                 for field in dt.fields {
    //                     if field.name == field_name {
    //                         return Ok(field);
    //                     }
    //                 }

    //                 return Err(anyhow!("Failed to locate information for structure field {}", field_name));
    //             }
    //             else {
    //                 return Err(anyhow!("Failed to locate information for structure field {}", field_name));
    //             }

    //         }

    //     }
    //     else {
    //         return Err(anyhow!("Cannot location any information for symbol {}", symbol_name));
    //     }

    //}
}

/// Extract a string from a byte stream. If the string cannot be
/// extracted, a blank string is returned.
fn extract_string_from_stream(bytes: &Vec<u8>) -> String {
    if let Ok(str) = std::str::from_utf8(bytes) {
        return str.to_string();
    } else {
        return String::new();
    }
}

/// A node for storing symbol information in a list or HashMap.
/// If a node is type Item, then it contains the information of
/// an actual symbol. If the node is type Domain, then it contains
/// a list of symbols under that domain.
pub enum AdsSymbolNode {
    Null,
    Item(AdsSymbolInfo),
    Domain(Box<HashMap<String, AdsSymbolNode>>),
}

fn parse_datatype_entry_field(
    item: &AdsDatatypeEntry,
    buffer: &Box<[u8]>,
    offset: usize,
) -> Option<AdsSymbolInfo> {
    let datatype_entry_len: usize = std::mem::size_of::<AdsDatatypeEntry>().try_into().unwrap();

    if offset >= (buffer.len() + datatype_entry_len) {
        return None;
    }

    let name_start = offset + datatype_entry_len;
    let name_end = name_start + item.nameLength as usize;

    if let Ok(nm) = String::from_utf8(buffer[name_start..name_end].to_vec()) {
        let mut ret = AdsSymbolInfo {
            name: nm,
            group_index: 0,
            index_offset: 0,
            size: item.size,
            type_id: item.dataType,
            type_name: String::new(),
            comment: String::new(),
            is_structure: item.subItems > 0,
            is_array: false,
            array_dimensions: Vec::new(),
        };

        let type_start = name_end + 1;
        let type_end = type_start + item.typeLength as usize;

        let comment_start = type_end + 1;
        let comment_end = comment_start + item.commentLength as usize;

        if item.typeLength > 0 {
            if let Ok(cmt) = String::from_utf8(buffer[type_start..type_end].to_vec()) {
                ret.type_name = cmt;
            }
        }
        if item.commentLength > 0 {
            if let Ok(cmt) = String::from_utf8(buffer[comment_start..comment_end].to_vec()) {
                ret.comment = cmt;
            }
        }

        ret.is_array = ret.type_name.contains("ARRAY");
        if ret.is_array {
            set_symbol_array_length(&mut ret);
        }

        return Some(ret);
    } else {
        return None;
    }
}

fn parse_datatype_entry_item(
    item: &AdsDatatypeEntry,
    buffer: &Box<[u8]>,
    offset: usize,
) -> Option<AdsDataTypeInfo> {
    let datatype_entry_len: usize = std::mem::size_of::<AdsDatatypeEntry>().try_into().unwrap();

    if offset >= (buffer.len() + datatype_entry_len) {
        return None;
    }

    // let item:&AdsDatatypeEntry = unsafe{ &*buffer[offset..offset + datatype_entry_len].as_ptr().cast() };

    let comment_len = item.commentLength;
    let name_start = offset + datatype_entry_len;
    let name_end = name_start + item.nameLength as usize;

    if let Ok(nm) = String::from_utf8(buffer[name_start..name_end].to_vec()) {
        let mut ret = AdsDataTypeInfo {
            name: nm.clone(),
            comment: String::new(),
            entry_length: item.entryLength,
            version: item.version,
            size: item.size,
            offset: item.offs,
            data_type: item.dataType,
            flags: item.flags,
            num_array_dimensions: item.arrayDim,
            num_fields: item.subItems,
            array_data_size: 0,
            array_dimensions: Vec::new(),
            fields: Vec::new(),
        };

        let type_start = name_end + 1;
        let type_end = type_start + item.typeLength as usize;

        let comment_start = type_end + 1;
        let comment_end = comment_start + item.commentLength as usize;

        if comment_len > 0 {
            if let Ok(cmt) = String::from_utf8(buffer[comment_start..comment_end].to_vec()) {
                ret.comment = cmt;
            }
        }

        //
        // Information about array dimensions follows the comment section.
        //
        if ret.num_array_dimensions > 0 {
            let mut array_info_offset = comment_end + 1;
            let array_info_len: usize = std::mem::size_of::<AdsDatatypeArrayInfo>()
                .try_into()
                .unwrap();

            let mut arr_size = 1;
            for _i in 0..ret.num_array_dimensions {
                let array_info_end = array_info_offset + array_info_len;
                let arr_info_item: &AdsDatatypeArrayInfo =
                    unsafe { &*buffer[array_info_offset..array_info_end].as_ptr().cast() };

                arr_size *= arr_info_item.elements;

                ret.array_dimensions.push(arr_info_item.elements as usize);

                array_info_offset += array_info_len;
            }

            ret.array_data_size = arr_size as usize;
        }

        //
        // Pull out information about any fields in the structure, which will be located
        // just after any array information and is a series of data type entries.
        //
        if ret.num_fields > 0 {
            let array_info_len: usize = std::mem::size_of::<AdsDatatypeArrayInfo>()
                .try_into()
                .unwrap();
            let field_info_offset =
                comment_end + 1 + (ret.num_array_dimensions as usize * array_info_len as usize);

            let mut field_item_offset = field_info_offset;
            for _i in 0..ret.num_fields {
                //log::info!("\tProcessing field item: {} of {}", i, ret.num_fields);

                let entry_end = field_item_offset + datatype_entry_len;
                let field_item: &AdsDatatypeEntry =
                    unsafe { &*buffer[field_item_offset..entry_end].as_ptr().cast() };

                if let Some(entry) =
                    parse_datatype_entry_field(field_item, buffer, field_item_offset)
                {
                    ret.fields.push(entry);
                } else {
                    ret.fields.push(AdsSymbolInfo::new());
                }

                field_item_offset += field_item.entryLength as usize;
            }
        }

        return Some(ret);
    } else {
        return None;
    }
}

fn parse_datatypes(buffer: &Box<[u8]>) -> BTreeMap<String, AdsDataTypeInfo> {
    let mut ret = BTreeMap::new();

    let mut offset = 0;
    let datatype_entry_len: usize = std::mem::size_of::<AdsDatatypeEntry>().try_into().unwrap();

    let mut num_types = 0;
    //
    // Cycle through and parse out all types in the PLC.
    //
    while offset < buffer.len() {
        if let Some(inc) = u32::read_from(&buffer[offset..offset + 4]) {
            let item: &AdsDatatypeEntry =
                unsafe { &*buffer[offset..offset + datatype_entry_len].as_ptr().cast() };

            if let Some(entry) = parse_datatype_entry_item(&item, buffer, offset) {
                ret.insert(entry.name.clone(), entry);
            } else {
                log::error!("Failed to parse a type!!");
            }

            // The inc and entryLength should be the same. The canonical example from
            // Beckhoff uses the calculated increment, so we maintain that practice.
            offset += inc as usize;
            num_types += 1;
        } else {
            break;
        }
    }

    log::info!("Uploaded {} data types from the PLC.", num_types);

    return ret;
}

/// Parse an array range declration "X...Y" to get the number of elements in an array.
fn parse_array_range(declaration: &str) -> usize {
    let tokens: Vec<&str> = declaration.split("..").collect();

    if tokens.len() < 2 {
        return 0;
    } else {
        if let Ok(lower) = tokens[0].parse::<usize>() {
            if let Ok(upper) = tokens[1].parse::<usize>() {
                return (upper - lower) + 1;
            }
        }
    }

    return 0;
}

/// Parse out the data type declaration to determine the array dimensions and
/// update the array_dimensions field in the provided AdsSymbolInfo.
/// # Arguments
/// * `symbol_info` - The AdsSymbolInfo to update.
fn set_symbol_array_length(symbol_info: &mut AdsSymbolInfo) {
    symbol_info.array_dimensions.clear();

    if !symbol_info.type_name.contains("ARRAY")
        || !symbol_info.type_name.contains("[")
        || !symbol_info.type_name.contains("]")
        || !symbol_info.type_name.contains("..")
    {
        return;
    }

    let start;
    let end;

    if let Some(index) = symbol_info.type_name.find("[") {
        start = index + 1;
    } else {
        return;
    }

    if let Some(index) = symbol_info.type_name.find("]") {
        end = index;
    } else {
        return;
    }

    if let Some(mid) = symbol_info.type_name.find(",") {
        // 2D array

        let first = symbol_info.type_name[start..mid].to_string();
        let second = symbol_info.type_name[mid + 1..end].to_string();

        symbol_info.array_dimensions.push(parse_array_range(&first));
        symbol_info
            .array_dimensions
            .push(parse_array_range(&second));
    } else {
        let range = symbol_info.type_name[start..end].to_string();
        symbol_info.array_dimensions.push(parse_array_range(&range));
    }
}

/// Returns a HashMap with symbol information uploaded from the remote
/// controller. All the symbols are uploaded and sorted into domains.
///
/// # Arguments
/// * `ams_address` - The ams address of the unit from which to upload.
/// * `port` - The communication port returned by AdsOpenPortEx
///
pub fn upload_symbols(ams_address: &AmsAddr, port: i32) -> Option<AdsSymbolCollection> {
    //
    // Create an instance of SymbolUploadInfo the controller can use
    //
    let mut upload_info = AdsSymbolUploadInfo2 {
        nSymbols: 0,
        nSymSize: 0,
        nDatatypes: 0,
        nDatatypeSize: 0,
        nMaxDynSymbols: 0,
        nUsedDynSymbols: 0,
    };
    let ptr_upload_info = &mut upload_info as *mut AdsSymbolUploadInfo2 as *mut c_void;

    //
    // Create a u32 instance we can use for receiving the bytes_read value back from the DLL.
    //
    let mut bytes_read: u32 = 0;
    let ptr_bytes_read = &mut bytes_read as *mut u32;

    //
    // Create a mutable ams_address, and a pointer to the that address
    // we can pass to the DLL.
    //
    let mut private_address = ams_address.clone();
    let raw_address = &mut private_address as *mut AmsAddr;

    // Read the length of the variable declaration

    unsafe {
        let read_len_err = AdsSyncReadReqEx2(
            port,
            raw_address,
            ADSIGRP_SYM_UPLOADINFO2,
            0x0,
            std::mem::size_of::<AdsSymbolUploadInfo2>()
                .try_into()
                .unwrap(),
            ptr_upload_info,
            ptr_bytes_read,
        );

        if read_len_err != 0 {
            log::error!(
                "Failed to upload SymbolUploadInfo2 with error {}",
                read_len_err
            );
            return None;
        }
    }

    log::info!(
        "The controller contains {} symbols with a total size of {} and data type size of {}",
        { upload_info.nSymbols },
        { upload_info.nSymSize },
        { upload_info.nDatatypeSize }
    );

    let total_symbol_size: usize = upload_info.nSymSize.try_into().unwrap();

    // create a buffer to store the symbol information.
    let mut buffer: Box<[u8]> = vec![0; total_symbol_size].into_boxed_slice();

    // let ptr_buffer = &mut buffer[0] as *mut [u8] as *mut c_void;
    // log::info!("The controller contains {} symbols with a total size of {}",
    //     {upload_info.nSymbols},
    //     {buffer.len()}
    // );

    let ptr_buffer = buffer.as_mut_ptr() as *mut c_void;

    // Now upload all symbol information in one large chunk.

    unsafe {
        // Read information about the PLC variables
        let symbol_upload_err = AdsSyncReadReqEx2(
            port,
            raw_address,
            ADSIGRP_SYM_UPLOAD,
            0,
            upload_info.nSymSize,
            ptr_buffer,
            ptr_bytes_read,
        );

        if symbol_upload_err != 0 {
            log::error!(
                "Failed to upload symbol information with error {}",
                symbol_upload_err
            );
            return None;
        }
    }

    let mut offset: usize = 0;
    let len: usize = std::mem::size_of::<AdsSymbolEntry>().try_into().unwrap();

    let mut uploaded_symbols: Vec<AdsSymbolInfo> =
        Vec::with_capacity(upload_info.nSymbols as usize);

    for _i in 0..upload_info.nSymbols {
        // let offset = i as usize * std::mem::size_of::<AdsSymbolEntry>();

        let item: &AdsSymbolEntry = unsafe { &*buffer[offset..offset + len].as_ptr().cast() };

        let mut symbol_info = AdsSymbolInfo::new();

        symbol_info.group_index = item.iGroup;
        symbol_info.index_offset = item.iOffs;
        symbol_info.size = item.size;
        symbol_info.type_id = item.dataType;

        // Strings related to the symbol are tacked on to the end of the AdsSymbolEntry.
        // That's why the offset needs to be incremented by the entryLength;
        //
        // These are the offsets from the TcAdsDef.h header from Beckhoff.
        //
        // #define	PADSSYMBOLNAME(p)			((char*)(((PAdsSymbolEntry)p)+1))
        // #define	PADSSYMBOLTYPE(p)			(((char*)(((PAdsSymbolEntry)p)+1))+((PAdsSymbolEntry)p)->nameLength+1)
        // #define	PADSSYMBOLCOMMENT(p)		(((char*)(((PAdsSymbolEntry)p)+1))+((PAdsSymbolEntry)p)->nameLength+1+((PAdsSymbolEntry)p)->typeLength+1)

        // #define	PADSNEXTSYMBOLENTRY(pEntry)	(*((unsigned long*)(((char*)pEntry)+((PAdsSymbolEntry)pEntry)->entryLength)) \
        //                         ? ((PAdsSymbolEntry)(((char*)pEntry)+((PAdsSymbolEntry)pEntry)->entryLength)): NULL)

        let symbol_name_offset = offset + len;
        let type_name_offset = symbol_name_offset + (item.nameLength as usize) + 1;
        let comment_offset = type_name_offset + (item.typeLength as usize) + 1;

        let name_bytes =
            buffer[symbol_name_offset..symbol_name_offset + item.nameLength as usize].to_vec();
        let type_bytes =
            buffer[type_name_offset..type_name_offset + item.typeLength as usize].to_vec();
        let comment_bytes =
            buffer[comment_offset..comment_offset + item.commentLength as usize].to_vec();

        symbol_info.name = extract_string_from_stream(&name_bytes);
        symbol_info.type_name = extract_string_from_stream(&type_bytes);
        symbol_info.comment = extract_string_from_stream(&comment_bytes);

        symbol_info.is_array = symbol_info.type_name.contains("ARRAY");
        if symbol_info.is_array {
            set_symbol_array_length(&mut symbol_info);
        }

        uploaded_symbols.push(symbol_info);

        offset += item.entryLength as usize;
    }

    let mut ret = AdsSymbolCollection::from_vector(&uploaded_symbols);
    // get size of datatype description
    let datatype_size = upload_info.nDatatypeSize;

    unsafe {
        // create a buffer to store the data type information.
        let mut data_type_buffer: Box<[u8]> = vec![0; datatype_size as usize].into_boxed_slice();
        let ptr_data_type_buffer = data_type_buffer.as_mut_ptr() as *mut c_void;

        if datatype_size > 0 {
            // upload data type descriptions
            let dtype_err = AdsSyncReadReqEx2(
                port,
                raw_address,
                ADSIGRP_SYM_DT_UPLOAD,
                0,
                upload_info.nDatatypeSize,
                ptr_data_type_buffer,
                ptr_bytes_read,
            );

            if dtype_err == ADSERR_NOERR as i32 {
                let dt_tree = parse_datatypes(&data_type_buffer);
                ret.data_types = dt_tree;
            }
        }
    }

    return Some(ret);
}

impl AdsSymbolCollection {
    /// Constructor. Returns an empty collection.
    pub fn new() -> AdsSymbolCollection {
        return AdsSymbolCollection {
            symbols: BTreeMap::new(),
            data_types: BTreeMap::new(),
        };
    }

    /// Search through the collection to find all items belonging to a particular
    /// domain. This method always returns a vector. If no tags exist for the
    /// provided domain, an empty vector is returned.
    ///
    /// # Arguments
    /// * `domain` - The domain to seek. Examples: GM, GIO.AXIS_PRESS
    pub fn find_symbols_in_domain(&self, domain: &str) -> Vec<AdsSymbolInfo> {
        let mut ret: Vec<AdsSymbolInfo> = Vec::new();

        for item in &self.symbols {
            if item.0.starts_with(&domain) {
                ret.push(item.1.clone());
            }
        }

        return ret;
    }

    /// Create a new symbol collection from a Vector of symbols
    pub fn from_vector(symbols: &Vec<AdsSymbolInfo>) -> AdsSymbolCollection {
        let mut ret = AdsSymbolCollection {
            symbols: BTreeMap::new(),
            data_types: BTreeMap::new(),
        };

        for item in symbols {
            ret.symbols.insert(item.name.clone(), item.clone());
        }

        return ret;
    }

    /// Prints the symbols after sorting them alphabetically.
    pub fn print_sorted_symbol_list(&self) {
        for item in &self.symbols {
            println!("{}", item.0);
        }
    }
}
