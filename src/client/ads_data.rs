//
// Copyright (C) 2024 Automated Design Corp.. All Rights Reserved.
// Created Date: 2024-04-09 08:11:58
// -----
// Last Modified: 2024-04-10 16:34:12
// -----
// 
//

use mechutil::variant::VariantValue;
use zerocopy::{AsBytes, FromBytes};
use anyhow::{anyhow, Error};
use std::{collections::HashMap, convert::TryFrom};

use super::ads_symbol_loader::{AdsSymbolCollection, AdsSymbolInfo};


#[allow(dead_code)]
#[repr(u32)]
#[derive(Debug, Clone)]
pub enum AdsDataTypeId 
{

	Void = 0,
	Int8 = 16,
	UInt8 = 17,
	Int16 = 2,
	UInt16 = 18,
	Int32 = 3,
	UInt32 = 19,
	Int64 = 20,
	UInt64 = 21,
	Real32 = 4,
	Real64 = 5,
	String = 30,
	WString = 31,
	Real80 = 32,
	Bit = 33,
    /// Some sort of structure or object.
	BigType = 65,
	MaxTypes = 67,
}


/// Attempt to convert a u32 value to the proper AdsDataTypeId
impl TryFrom<u32> for AdsDataTypeId {
    type Error = &'static str;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(AdsDataTypeId::Void),
            16 => Ok(AdsDataTypeId::Int8),
            17 => Ok(AdsDataTypeId::UInt8),
            2 => Ok(AdsDataTypeId::Int16),
            18 => Ok(AdsDataTypeId::UInt16),
            3 => Ok(AdsDataTypeId::Int32),
            19 => Ok(AdsDataTypeId::UInt32),
            20 => Ok(AdsDataTypeId::Int64),
            21 => Ok(AdsDataTypeId::UInt64),
            4 => Ok(AdsDataTypeId::Real32),
            5 => Ok(AdsDataTypeId::Real64),
            30 => Ok(AdsDataTypeId::String),
            31 => Ok(AdsDataTypeId::WString),
            32 => Ok(AdsDataTypeId::Real80),
            33 => Ok(AdsDataTypeId::Bit),
            65 => Ok(AdsDataTypeId::BigType),
            67 => Ok(AdsDataTypeId::MaxTypes),
            _ => Err("Invalid AdsDataTypeId"),
        }
    }
}



/// Convert a stream of bytes to a P.O.D. type that is compatible with
/// ZeroCopy contraints. Doesn't work for dynamic types.
pub fn value_from_bytes<T: FromBytes + Default>(bytes : &Vec<u8>) -> Option<T> 
where
    T: Default + AsBytes + FromBytes, // ZeroCopy constraints
{
    return T::read_from(&*bytes); 
}


/// Convert a vector of bytes to a string and trims off suplerflous bytes
/// sent by the PLC.
pub fn vec_to_string(s : &Vec<u8>) -> Result<String,anyhow::Error> {

    if let Some(end_index) = s.iter().position(|&c| c == 0x00) {
        match String::from_utf8(s[..end_index].to_vec()) {
            Ok(ret) => return Ok(ret),
            Err(err) => Err(anyhow!("Failed to convert string: {}", err)),
        }
    }
    else {
        return Err(anyhow!("String is not null-terminated."));
    }
}




pub fn deserialize_structure(
    symbol_collection: &AdsSymbolCollection,  
    symbol_info: &AdsSymbolInfo,
    bytes : &Vec<u8>, 
    type_id : &AdsDataTypeId 
) -> Option<VariantValue> {

    if let Some(dt) = symbol_collection.data_types.get(&symbol_info.type_name) {

        let mut map : HashMap<String, VariantValue> = HashMap::new();

        let mut offset: usize = 0;
        for field in &dt.fields {
            
            if let Some(field_info) = symbol_collection.data_types.get(&field.type_name) {
                
                if field_info.num_fields > 0 {
                    // This is a structure
                    if let Some(val) = deserialize_structure(
                        symbol_collection, 
                        symbol_info, 
                        &bytes[offset..offset + field.size as usize].to_vec(), 
                        type_id
                    ) {
                        map.insert(field.name.clone(), val);
                    }
                    
                }
                else if field_info.array_data_size > 0 {
                    match AdsDataTypeId::try_from(field.type_id) {
                        Ok(field_type_id) => {

                            let mut arr = Vec::new();
                        
                            let inner_array1 = VariantValue::Array(vec![VariantValue::Int32(1), VariantValue::Int32(2)]);
                            let inner_array2 = VariantValue::Array(vec![VariantValue::Int32(3), VariantValue::Int32(4)]);
                            let outer_array = VariantValue::Array(vec![inner_array1, inner_array2]);                        
                            

                            for col in &field_info.array_dimensions {
                                let mut array_elements = Vec::new();

                                for i in 0 .. *col {



                                    if let Some(val) = deserialize(
                                        symbol_collection, 
                                        symbol_info, 
                                        &bytes[offset..offset + field.size as usize].to_vec(),
                                        &field_type_id
                                    ) {
                                        array_elements.push(val);
                                    }
                                    else {
                                        array_elements.push(VariantValue::Null);
                                    }
                                }

                                arr.push(array_elements);
                            }




                            

                            map.insert(field.name.clone(), arr);
                        },
                        Err(err) => {
                            log::error!("Error processing field {} type {} err: {}", field.name, field.type_id, err);
                            map.insert(field.name.clone(), VariantValue::Null);
                        }                        
                    }
                }
                else {
                    // plain old data
                    match AdsDataTypeId::try_from(field.type_id) {
                        Ok(field_type_id) => {
                            if let Some(val) = deserialize(
                                symbol_collection, 
                                symbol_info, 
                                &bytes[offset..offset + field.size as usize].to_vec(), 
                                &field_type_id
                            ) {
                                map.insert(field.name.clone(), val);
                            }        
                        },
                        Err(err) => {
                            log::error!("Error processing field {} type {} err: {}", field.name, field.type_id, err);
                            map.insert(field.name.clone(), VariantValue::Null);
                        }
                    }
                }

            }


            offset += field.size as usize;


        }

        return Some(VariantValue::Object(Box::new(map)));

    }
    else {
        return None;
    }

    
}



pub fn deserialize_array(
    symbol_collection: &AdsSymbolCollection,  
    symbol_info: &AdsSymbolInfo,
    bytes : &Vec<u8>, 
    type_id : &AdsDataTypeId 
) {

}


/// Deserialize a byte stream into the appropriate type.
pub fn deserialize(
    symbol_collection: &AdsSymbolCollection,  
    symbol_info: &AdsSymbolInfo,
    bytes : &Vec<u8>, 
    type_id : &AdsDataTypeId 
) -> Option<VariantValue> {

    log::debug!("deserialize type id: {:?} is array {}", type_id, symbol_info.is_array);


    if symbol_info.is_array {
        return deserialize_structure(
            symbol_collection, 
            symbol_info, 
            bytes, 
            &type_id
        );
    }

    match type_id {
        AdsDataTypeId::Void => return None,
        AdsDataTypeId::Int8 => {
            if let Some(val) = value_from_bytes::<i8>(bytes) {
                return Some(VariantValue::from(val));
            } else {
                return None;    
            }
        },            
        AdsDataTypeId::UInt8 => {
            if let Some(val) = value_from_bytes::<u8>(bytes) {
                return Some(VariantValue::from(val));
            } else {
                return None;    
            }            
        },
        AdsDataTypeId::Int16 => {
            if let Some(val) = value_from_bytes::<i16>(bytes) {
                return Some(VariantValue::from(val));
            } else {
                return None;    
            }            
        },
        AdsDataTypeId::UInt16 => {
            if let Some(val) = value_from_bytes::<u16>(bytes) {
                return Some(VariantValue::from(val));
            } else {
                return None;    
            }            
        },
        AdsDataTypeId::Int32 => {
            if let Some(val) = value_from_bytes::<i32>(bytes) {
                return Some(VariantValue::from(val));
            } else {
                return None;    
            }            
        },
        AdsDataTypeId::UInt32 => {
            if let Some(val) = value_from_bytes::<u32>(bytes) {
                return Some(VariantValue::from(val));
            } else {
                return None;    
            }            
        },
        AdsDataTypeId::Int64 => {
            if let Some(val) = value_from_bytes::<i64>(bytes) {
                return Some(VariantValue::from(val));
            } else {
                return None;    
            }            
        },
        AdsDataTypeId::UInt64 => {
            if let Some(val) = value_from_bytes::<u64>(bytes) {
                return Some(VariantValue::from(val));
            } else {
                return None;    
            }            
        },
        AdsDataTypeId::Real32 => {
            if let Some(val) = value_from_bytes::<f32>(bytes) {
                return Some(VariantValue::from(val));
            } else {
                return None;    
            }            
        },
        AdsDataTypeId::Real64 => {
            if let Some(val) = value_from_bytes::<f64>(bytes) {
                return Some(VariantValue::from(val));
            } else {
                return None;    
            }            
        },
        AdsDataTypeId::String => {
            if let Ok(val) = vec_to_string(bytes) {
                return Some(VariantValue::from(val));
            }   
            else {
                return None;
            }
        },
        AdsDataTypeId::WString => return None,
        AdsDataTypeId::Real80 => return None,
        AdsDataTypeId::Bit => {
            if let Some(val) = value_from_bytes::<i8>(bytes) {
                return Some(VariantValue::from(val));
            } else {
                return None;    
            }               
        },
        AdsDataTypeId::BigType => {
            return deserialize_structure(
                symbol_collection, 
                symbol_info, 
                bytes, 
                &type_id
            );
        },
        AdsDataTypeId::MaxTypes => return None // Not an actual type; just a marker for maximum built-in types
    }

}