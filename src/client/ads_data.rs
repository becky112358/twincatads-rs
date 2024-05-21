//
// Copyright (C) 2024 Automated Design Corp.. All Rights Reserved.
// Created Date: 2024-04-09 08:11:58
// -----
// Last Modified: 2024-05-21 10:46:18
// -----
// 
//

use indexmap::IndexMap;
use mechutil::variant::VariantValue;
use zerocopy::{AsBytes, FromBytes};
use anyhow::anyhow;
use std::convert::TryFrom;
use serde::{Serialize, Deserialize};

use super::ads_symbol_loader::{AdsSymbolCollection, AdsSymbolInfo};


#[allow(dead_code)]
#[repr(u32)]
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    _type_id : &AdsDataTypeId 
) -> Option<VariantValue> {
    
    if let Some(dt) = symbol_collection.get_fundamental_type_info(&symbol_info) {

        let mut map : IndexMap<String, VariantValue> = IndexMap::new();

        for field in &dt.fields {

            // We have to use the specified field offset uploaded from the device. The
            // controller doesn't always align bytes as expected; it will add "dummy" bytes.
            let offs = field.offset as usize;
            
            if let Some(field_info) = symbol_collection.data_types.get(&field.type_name) {
                
                // log::debug!("*** field_info.num_fields > {} || field.is_structure {} || field.type_id == {}\n{:?}\n{:?}\n", 
                //     field_info.num_fields, field.is_structure, field.type_id, field, field_info.array_dimensions

                // );

                if field_info.num_fields > 0 || field.is_structure || (field.type_id == 65 && !field.is_array) {
                    // This is a structure

                    match AdsDataTypeId::try_from(field.type_id) {
                        Ok(field_type_id) => {

                            // log::debug!("Field {} is a structure with size {} [{}]", field.name, field.size, field_info.size);
                            
                            if let Some(val) = deserialize_structure(
                                symbol_collection, 
                                field, 
                                &bytes[offs..offs + field.size as usize].to_vec(), 
                                &field_type_id
                            ) {
                                map.insert(field.name.clone(), val);
                            }
                            else {
                                log::error!("Error deserializing field {} type {}", field.name, field.type_id);
                                map.insert(field.name.clone(), VariantValue::Null);                                
                            }
                        },
                        Err(err) => {
                            log::error!("Error processing type of field {} type {} err: {}", field.name, field.type_id, err);
                            map.insert(field.name.clone(), VariantValue::Null);
                        }                           
                    }


                    
                }
                else if field.is_array {

                    //log::debug!("Field {} is array.", field.name);

                    match AdsDataTypeId::try_from(field.type_id) {
                        Ok(field_type_id) => {

                            let arr = deserialize_array(
                                symbol_collection, 
                                field, 
                                &bytes[offs..offs + field.size as usize].to_vec(), 
                                &field_type_id
                            );

                            if let Some(var) = arr {
                                map.insert(field.name.clone(), var);
                            }
                            else {
                                map.insert(field.name.clone(), VariantValue::Null);
                            }
                            
                        },
                        Err(err) => {
                            log::error!("Error processing field {} type {} err: {}", field.name, field.type_id, err);
                            map.insert(field.name.clone(), VariantValue::Null);
                        }                        
                    }
                }
                else {
                    // plain old data

                    // log::debug!("P.O.D. Field offset {} type {}\n\t{:?}", field.offset, field.type_id, symbol_info);

                    match AdsDataTypeId::try_from(field.type_id) {
                        Ok(field_type_id) => {
                            if let Some(val) = deserialize_value(
                                symbol_collection, 
                                symbol_info, 
                                &bytes[offs..offs + field.size as usize].to_vec(), 
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


        }

        return Some(VariantValue::Object(Box::new(map)));

    }
    else {
        log::error!("Failed to find structure type name: {}", symbol_info.type_name);
        return None;
    }

    
}



pub fn deserialize_array(
    symbol_collection: &AdsSymbolCollection,  
    symbol_info: &AdsSymbolInfo,
    bytes : &Vec<u8>, 
    type_id : &AdsDataTypeId 
) -> Option<VariantValue> {

    if let Some(symbol_dt) = symbol_collection.data_types.get(&symbol_info.type_name) {

        let mut inc = 0;

        // Now let's get the type name
        if let Some(of) = symbol_dt.name.find("OF") {
            let val_type = symbol_info.type_name[of + 3..].to_string();
            if let Some(val_dt) = symbol_collection.data_types.get(&val_type) {
                inc = val_dt.size as usize;
            }
        }
        else {
            return None;
        }

    
        let mut arr = Vec::new();

        let mut offset = 0;

        for col in &symbol_info.array_dimensions {            

            let mut array_elements = Vec::new();
    
            for _i in 0 .. *col {

                if let Some(val) = deserialize_value(
                    symbol_collection, 
                    symbol_info, 
                    &bytes[offset..offset + inc as usize].to_vec(),
                    &type_id
                ) {
                    array_elements.push(val);
                }
                else {
                    array_elements.push(VariantValue::Null);
                }

                offset += inc;
            }
    
            arr.push(array_elements);
        }            


        if arr.len() > 1 {
            // 2D array
            let arr1 = VariantValue::Array(arr[0].clone());
            let arr2 = VariantValue::Array(arr[1].clone());
            return Some(VariantValue::Array(vec![arr1, arr2]));
        }
        else if arr.len() == 1 {
            return Some(VariantValue::Array(arr[0].clone()));
        }
        else {
            return None;
        }
        
    }
    else {
        return None;
    }

}


/// Deserialize a value, either P.O.D. or a structure.
pub fn deserialize_value(
    symbol_collection: &AdsSymbolCollection,  
    symbol_info: &AdsSymbolInfo,
    bytes : &Vec<u8>, 
    type_id : &AdsDataTypeId 
) -> Option<VariantValue> {    

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
                if val == 0 {
                    return Some(VariantValue::from(false));
                }
                else {
                    return Some(VariantValue::from(true));
                }                
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



/// Deserialize a byte stream into the appropriate type.
pub fn deserialize(
    symbol_collection: &AdsSymbolCollection,  
    symbol_info: &AdsSymbolInfo,
    bytes : &Vec<u8>, 
    type_id : &AdsDataTypeId 
) -> Option<VariantValue> {

    if symbol_info.is_array {
        return deserialize_array(
            symbol_collection, 
            symbol_info, 
            bytes, 
            &type_id
        );
    }
    else {
        return deserialize_value(
            symbol_collection, 
            symbol_info, 
            bytes, 
            &type_id
        );
    }
}

/// Serialize an array variant properly for the controller, which will not be byte-aligned as compactly
/// as the default VariantValue serialize functions will be. This function uses information
/// from the symbol table to align values properly.
pub fn serialize_array(
    symbol_collection: &AdsSymbolCollection,  
    symbol_info: &AdsSymbolInfo,    
    value : &VariantValue
) -> Result<Vec<u8>, anyhow::Error> {

    match value {
        VariantValue::Array(arr) => {
            let mut all_bytes = Vec::new();

            for item in arr {
                match serialize(symbol_collection, symbol_info, item) {
                    Ok(bytes) => {
                        all_bytes.extend(bytes);
                    },
                    Err(err) => {
                        return Err(anyhow!("Error serializing an array index : {}", err));
                    }
                }
            }

            if all_bytes.len() < symbol_info.size as usize {
                all_bytes.resize(symbol_info.size as usize, 0);
            }        

            Ok(all_bytes)
        },
        _ => Err(anyhow!("Expected VariantValue::Object for serialization")),
    }

}


/// Serialize a structured variant properly for the controller, which will not be byte-aligned as compactly
/// as the default VariantValue serialize functions will be. This function uses information
/// from the symbol table to align values properly.
pub fn serialize_struct(
    symbol_collection: &AdsSymbolCollection,  
    symbol_info: &AdsSymbolInfo,    
    value : &VariantValue) -> Result<Vec<u8>, anyhow::Error> {

    match value {
        VariantValue::Object(boxed_map) => {
            let mut all_bytes = Vec::new();

            if let Some(dt) = symbol_collection.get_fundamental_type_info(symbol_info) {

                let mut count = 0;
                // Iterate over the IndexMap and serialize each key-value pair
                for (key, val) in boxed_map.iter() {

                    if count < dt.fields.len() {
                        let field = &dt.fields[count];                                            

                        match serialize(symbol_collection, field, val) {
                            Ok(bytes) => {
                                let offs = field.offset as usize;
                                if all_bytes.len() < offs {
                                    // The structure has been padded with "dummy" bytes in the controller.
                                    // Adjust.
                                    // Calculate the number of padding bytes needed
                                    let padding_size = offs - all_bytes.len();
                                    
                                    // Extend all_bytes with the calculated number of zero bytes
                                    all_bytes.extend(std::iter::repeat(0).take(padding_size));                                    
                                }

                                all_bytes.extend(bytes);
                            },
                            Err(err) => {
                                return Err(anyhow!("Error serializing field {} : {}", key, err));
                            }
                        }
    
                        count += 1;    
                    }
                    else {
                        return Err(anyhow!("count {} >= files length {}", count, dt.fields.len()));
                    }
                }

                if all_bytes.len() < dt.size as usize {
                    all_bytes.resize(dt.size as usize, 0);
                }                
    
                Ok(all_bytes)                

            }
            else {
                return Err(anyhow!("Failed to get type information for structure {}", symbol_info.name));
            }

        },
        _ => Err(anyhow!("Expected VariantValue::Object for serialization")),
    }

}




/// Serialize a variant properly for the controller, which will not be byte-aligned as compactly
/// as the default VariantValue serialize functions will be. This function uses information
/// from the symbol table to align values properly.
pub fn serialize(
    symbol_collection: &AdsSymbolCollection,  
    symbol_info: &AdsSymbolInfo,
    value : &VariantValue) -> Result<Vec<u8>, anyhow::Error> {
    
    match &*value {
        VariantValue::Array(_) => {

            return serialize_array(symbol_collection, symbol_info,value);

        }
        VariantValue::Object(_) => {
            
            return serialize_struct(symbol_collection, symbol_info,value);

        },
        VariantValue::String(str) => {
            let mut ret = str.as_bytes().to_vec();
            
            // We need to ensure that the string is both not longer than
            // the allocated space, but also wipes out any characters that might
            // be longer than the previous string.
            ret.resize(symbol_info.size as usize, 0);
            return Ok(ret);
        },
        _ =>  return serialize_value(value)
    }    
    
}




/// Serialize a POD value into a byte stream that can be transmitted
/// over a connection.
fn serialize_value(value : &VariantValue) -> Result<Vec<u8>, anyhow::Error> {


    match &*value {
        VariantValue::Null => {
            return Err(anyhow!("Cannot serialize a NULL value!"));
        },
        VariantValue::Bit (s) =>{
            if *s {
                return Ok(vec![1]);
            }
            else {
                return Ok(vec![0]);
            }
        },  
        VariantValue::Byte (s) =>{
            return Ok(vec![*s]);
        },  
        VariantValue::SByte (s) =>{
            return Ok(vec![*s as u8]);
        },  
        VariantValue::Int16 (s) =>{
            return Ok(s.to_le_bytes().to_vec());
        },  
        VariantValue::UInt16 (s) =>{
            return Ok(s.to_le_bytes().to_vec());
        },  
        VariantValue::Int32 (s) =>{
            return Ok(s.to_le_bytes().to_vec());
        },  
        VariantValue::UInt32 (s) =>{
            return Ok(s.to_le_bytes().to_vec());
        },  
        VariantValue::Int64 (s) =>{
            return Ok(s.to_le_bytes().to_vec());
        },  
        VariantValue::UInt64 (s) =>{
            return Ok(s.to_le_bytes().to_vec());
        },  
        VariantValue::Real32 (s) =>{
            return Ok(s.to_le_bytes().to_vec());
        },  
        VariantValue::Real64 (s) =>{
            return Ok(s.to_le_bytes().to_vec());
        },  
        _ => {
            return Err(anyhow!("This function not intended for all but POD types."));
        }
    
    }
    
}