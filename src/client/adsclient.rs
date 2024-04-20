#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

/*
 * (C) Copyright 2022-2024 Automated Design Corp. All Rights Reserved.
 */

//! ADS Client that wraps the API from the Beckhoff DLL into a convenient
//! struct with commonly-used features. Includes registering and handling
//! notifications from the target, serilization and deserialization of
//! values and thread-safe channels. The ads_symbol_loader module is used
//! to upload symbol information from the target. 


use std::collections::HashMap;
use tokio::task::JoinHandle;
use tokio::sync::mpsc::{self, Sender, Receiver};
use std::time;
use std::slice;
use std::os::raw::c_void; // c_void, etc
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use log::{error, info};
use std::str::FromStr;
use lazy_static::lazy_static;

use zerocopy::{AsBytes, FromBytes};
use anyhow::anyhow;


use mechutil::notifier::Notifier;
use mechutil::variant::{self, VariantValue};

use crate::client::ads_symbol_loader;
use crate::client::client_types::EventInfoType;
use crate::{AdsAmsRegisterRouterNotification, AdsAmsUnRegisterRouterNotification, AmsAddr};
use crate::AdsSymbolEntry;
use crate::AdsNotificationHeader;
use crate::AdsGetLocalAddress;
use crate::AdsPortCloseEx;
use crate::AdsPortOpenEx;
use crate::AdsSyncReadWriteReqEx2;
use crate::AdsSyncWriteReqEx;
use crate::AdsSyncReadReqEx2;
use crate::AdsSyncAddDeviceNotificationReqEx;
use crate::AdsSyncDelDeviceNotificationReqEx;
use crate::AdsGetDllVersion;
use crate::ADSIGRP_SYM_HNDBYNAME;
use crate::ADSIGRP_SYM_INFOBYNAMEEX;
use crate::ADSIGRP_SYM_VALBYHND;
use crate::AdsNotificationAttrib__bindgen_ty_1;
use crate::nAdsTransMode_ADSTRANS_SERVERONCHA;
use crate::AdsNotificationAttrib;
use crate::ADSIGRP_DEVICE_DATA;
use crate::ADSIOFFS_DEVDATA_ADSSTATE;

use super::ads_symbol_loader::{AdsSymbolCollection, AdsSymbolInfo};
use super::client_types::{
    AdsClientNotification, RegisteredSymbol, RouterNotificationEventInfo
};

use super::ads_data::{self, serialize, vec_to_string, AdsDataTypeId};



#[allow(dead_code)]
struct SymbolManager {
    /// Symbols that have been registered for on-data noticifcation.
    /// The key is the handle (not the notification handle) for the symbol
    /// that was collected in the first step of registering the symbol.
    pub registered_symbols : Mutex<Vec<RegisteredSymbol>>,
    pub notify : Mutex<Notifier::<'static, RouterNotificationEventInfo>>,
    //pub data_change : Mutex<DataChangeCallback>,
    pub sender : Mutex<Vec<Sender<RouterNotificationEventInfo>>>,
    pub receiver : Mutex<Option<Receiver<RouterNotificationEventInfo>>>,
    pub is_running : Mutex<bool>

}

lazy_static! {
    /// Shared instance of the symbols
    static ref SYMBOL_MANAGER : SymbolManager = SymbolManager { 
        registered_symbols: Mutex::new(vec![]),
        notify : Mutex::new(Notifier {
            subscribers: Vec::new(),
        }),
        //data_change : Mutex::new(DataChangeCallback::new()),
        sender : Mutex::new(vec![]),
        receiver : Mutex::new(None),
        is_running : Mutex::new(false)
    };

}



/// Locate a symbol registered for on value change notification by its name.
fn get_registered_symbol_by_name( symbol_name : &str ) -> Result<RegisteredSymbol, anyhow::Error> {

    let guard = SYMBOL_MANAGER.registered_symbols.lock().unwrap();


    for i in 0 .. guard.len() {
        let item = &guard[i];

        if item.name == symbol_name {
            return Ok(item.clone());
        }

    }

    return Err(anyhow!("Symbol name {} is not registered in the notification collection.", symbol_name));
}


/// Remove a symbol registered for on value change notification from the list.
fn remove_registered_symbol_by_name( symbol_name : &str ) -> Result<(), anyhow::Error> {

    let mut guard = SYMBOL_MANAGER.registered_symbols.lock().unwrap();

    for i in 0 .. guard.len() {
        let item = &guard[i];

        if item.name == symbol_name {
            guard.remove(i);
            return Ok(());
        }

    }

    return Err(anyhow!("Failed to locate symbol name {} to remove.", symbol_name));

}


/// Finds and returns the symbol registered for on value change notification from the global list.
fn get_registered_symbol_by_handle( handle : u32) -> Result<RegisteredSymbol, anyhow::Error> {


    let guard = SYMBOL_MANAGER.registered_symbols.lock().unwrap();


    for i in 0 .. guard.len() {
        let item = &guard[i];

        if item.handle == handle {
            return Ok(item.clone());
        }
    }

    return Err(anyhow!("Symbol handle {} is not registered in the notification collection.", handle));
}

/// Get the last symbol stored in the collection, if it exists.
fn get_last_registered_symbol() -> Option<RegisteredSymbol> {
    let guard = SYMBOL_MANAGER.registered_symbols.lock().unwrap();

    if guard.len() > 0 {
        return guard.last().clone().cloned();
    }
    else {
        return None;
    }
}

/// Add a symbol to the list of registered symbols used when on value changed notifications are received.
fn add_registered_symbol(s :RegisteredSymbol ) {

    let mut mutex = SYMBOL_MANAGER.registered_symbols.lock().unwrap();

    info!("Adding register symbol {} with handle {} ", s.name, s.handle);
    mutex.push(s);
}



/// Returns an id to identify the AdsClient instance. The same
/// ID is never use twice in the same run-time.
fn get_new_id() -> u32 {
    static COUNTER:AtomicU32 = AtomicU32::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}





/// Establishes and manages a connection to a target device (i.e. the PLC) via the 
/// TwinCAT ADS Router. This is a wrapper for the TwinCAT DLL functions that provides
/// commonly-required features.
/// 
/// Before making a connection, be sure to have established the router using the
/// ADS Router->Edit Routes dialog.
/// 
/// Simple example:
/// ```
/// let (mut client, rx) = AdsClient::new();
/// // Supply AMS Address. If not set, localhost is used.
/// client.set_address("192.168.127.1.1.1");
/// // Supply ADS port. If not set, the default of 851 is used.
/// // You should generally use the default.
/// client.set_port(851);
/// 
/// // Make the connection to the ADS router
/// client.initialize();
/// 
/// // Write a value to a symbol in the PLC
/// if let Err(err) = client.write_symbol_value(
///     "GVL.fMyCoolValue",
///     1.234
/// ) {
///     println!("An error occurred writing the tag: {}", err);
/// }
/// 
/// // On-change notification
/// const NOTIFY_TAG :&str = "GM.fReal";
///     
/// if let Err(err) = client.register_symbol(NOTIFY_TAG) {
///     println!("Failed to register symbol: {}", err);
/// }
/// else {
/// 
/// 
///     // Listen for notifications
///     thread::spawn(move || {
/// 
///         //
///         // When you call .iter() on a Receiver, you get an iterator that blocks waiting for new messages, 
///         // and it continues until the channel is closed.
///         //
///         for notification in rx.iter() {
///             println!("Notification received: {} : {}", notification.name, notification.value);
///             // Handle the notification...
///         }
///     });
/// 
/// 
///     if let Err(err) = client.unregister_symbol(NOTIFY_TAG) {
///         println!("Failed to unregister symbol: {} ", err);
///     }
/// }
/// 
/// // Make sure the ADS client is closed.
/// client.finalize();
/// 
/// ```
pub struct AdsClient {

    /// The INSTANCE id of this instance. Used with the C callbacks.
    id : u32,

    /// target address of the remote PLC.
    address : AmsAddr,    // netId, port


    /// The communications port to the ADS router. This is
    /// not the same as a connection to the PLC, but is a
    /// required first step.
    current_comms_port : i32,
    
    symbol_collection : AdsSymbolCollection,

    /// A buffer of symbol entries uploaded from the PLC. Symbol Entry information
    /// is used for synchronous reads and writes. This map must be cleared when the 
    /// connection is lost, when the PLC is re-started or when an the project is re-actviated.
    symbol_entries : HashMap<String, AdsSymbolEntry>,

    /// Notification channel back to the parent that owns this instance of the AdsClient.
    notification_tx: mpsc::Sender<AdsClientNotification>,

    /// Stores the join handle for the notification thread.
    notification_join_handle : Option<JoinHandle<()>>,

    /// Signal used to shut down the message loop that handles notifications between 
    /// threads.
    shutdown_signal : Arc<AtomicBool>,

    /// Notification handle for the ADS State callback.
    ads_state_notification_handle : u32

}

impl AdsClient {

    /// Constructor
    pub fn new() -> (Self, mpsc::Receiver<AdsClientNotification>) {

        let (tx, rx) = mpsc::channel(100);

        let client = AdsClient { 
            id : get_new_id(),
            address : get_local_address(),
            current_comms_port : 0,
            symbol_collection : AdsSymbolCollection::new(),
            symbol_entries : HashMap::new(),
            notification_tx: tx,
            shutdown_signal : Arc::new(AtomicBool::new(false)),
            notification_join_handle: None,
            ads_state_notification_handle: 0,            
        };


        return (client, rx);
    }


    /// Set the target address of the device.
    pub fn set_address(&mut self, s : &str) {

        if s.len() > 0 {
            if let Some(addr) = to_ams_addr(s) {
                self.address = addr;
            }
            else {
                log::warn!("Could not parse address as provided: {}", s);                
                self.address = get_local_address();
                log::warn!("Defaulting to local address.");
            }    
        }      
        else {
            //
            // A blank entry is an intentional selection of the local address.
            //            
            self.address = get_local_address();
            log::info!("ADS Client using local address.");
        }
    }


    /// Set the target ams port of the device.
    /// The "port" number identifies different tasks and components in a
    /// TwinCAT runtime.
    /// 
    /// The default is 851, which is usually correct.
    pub fn set_port(&mut self, s : u16) {
        self.address.port = s;
    }

    /// Spin up the Ads library and open a 
    /// port to the router. Opening the port
    /// isn't the same as opening the connection, but
    /// it's a required first step.
    pub fn initialize(&mut self) {

        self.symbol_entries.clear();

        if self.current_comms_port != 0 {
            unsafe {
                AdsPortCloseEx(self.current_comms_port);
            }
            self.current_comms_port = 0;
        }

        unsafe {
            
            // Open a legacy function port, which is used by the router notification command.
            // Most of the other command use the multi-threaded version of the commands
            crate::AdsPortOpen();


            self.current_comms_port = AdsPortOpenEx();
        }

        //
        // Create communication channels.
        //

        let (tx, mut rx): (Sender<RouterNotificationEventInfo>, Receiver<RouterNotificationEventInfo>) 
            = mpsc::channel(100);

        let mut global_sender = SYMBOL_MANAGER.sender.lock().unwrap();
        global_sender.push(tx);


        //
        // Buffer all the symbol information from the PLC.
        //
        self.upload_all_symbols();


        let notification_tx = self.notification_tx.clone();
        let shutdown_signal_clone = self.shutdown_signal.clone();
        let symbol_collection = self.symbol_collection.clone();

        let child = tokio::task::spawn(async move {

            let ten_millis = time::Duration::from_millis(10);
            let timeout = time::Duration::from_secs(1); // Set your desired timeout
            //let mut ads_state : i16 = -1;
            //let mut router_state : i16 = -1;        

            loop {
            
                if shutdown_signal_clone.load(Ordering::SeqCst) {
                    info!("AdsClient: shutdown signal received, exiting notification thread.");
                    break; 
                }                

                let result = {
                    
                    tokio::time::timeout(timeout, rx.recv()).await
                };


                match result  { 

                    Ok(opt) => {
                        match opt {
                            Some(info) => {
                                match info.event_type {
                                    EventInfoType::DataChange => {
                                        if let Ok(symbol) = get_registered_symbol_by_handle(info.id) {

                                            // log::debug!("CHANNEL DATA NOTIFICATION: {} event received for symbol: {}", 
                                            //     info.id,
                                            //     symbol.name
                                            // );

                                            if let Some(symbol_info) = symbol_collection.symbols.get(&symbol.name) {
                                                match ads_data::deserialize(
                                                    &symbol_collection,
                                                    symbol_info,
                                                    &info.data, 
                                                    &symbol.data_type_id
                                                ) {
                                                    Some(var) => {
                    
                                                        let notification = AdsClientNotification::new_datachange(&symbol.name, &var);
                                                        
                                                        if let Err(err) = notification_tx.send(notification).await {
                                                            error!("Failed to send notification on {} to parent with error: {}", 
                                                                symbol.name, 
                                                                err
                                                            );
                                                        }
                    
                                                    },
                                                    None => {
                                                        log::error!("Failed to create Variant for notification on symbol {}", symbol.name);
                                                    }
                                                }
                    
                                            }
                                            else {
                                                log::error!("Failed to locate symbol name {} in uploaded symbol table.", symbol.name);
                                            }

                                            
                                        }
                                        else {
                                            log::error!("Channel data notification received for unknown error.");
                                        }            
                                    }
                                    EventInfoType::Invalid => {
                                        log::warn!("Invalid notification received. Ignoring.");
                                    },
                                    EventInfoType::AdsState => {
                                        if let Some(state ) = i16::read_from(&info.data) {

                                            let notification = AdsClientNotification::new_ads_state(state);
                                                        
                                            if let Err(err) = notification_tx.send(notification).await {
                                                error!("Failed to send ads state notification to parent with error: {}", 
                                                    err
                                                );
                                            }                                    

                                            log::info!("PLC state is now: {}", state);
                                        }
                                        else {
                                            log::error!("Failed to extract PLC state from notification.");
                                        }
                                        
                                    },
                                    EventInfoType::RouterState => {
                                        if let Some(state ) = i16::read_from(&info.data) {

                                            let notification = AdsClientNotification::new_router_state(state);
                                                        
                                            if let Err(err) = notification_tx.send(notification).await {
                                                error!("Failed to send router state notification to parent with error: {}", 
                                                    err
                                                );
                                            }                                    

                                            log::info!("ADS Router is now: {}", state);
                                        }
                                        else {
                                            log::error!("Failed to extract PLC state from notification.");
                                        }
                                    },
                                }
                                let _ = tokio::time::sleep(ten_millis).await;          
                            },
                            None => {
                                info!("Notification Sender has disconnected or faulted. No more notifications will be received.");
                                break;    
                            }

                        };

                    },
                    Err(_) => {
                        // Timeout. Simply loop up.
                        let _ = tokio::time::sleep(ten_millis).await;
                    }
                }
            }            

        });

        self.notification_join_handle = Some(child);

        self.register_state_notification();
        self.register_ads_router_notification();



    }



    /// Complete the shutdown and closing the port to the
    /// ads router. This should be done last, after all 
    /// connections are cleaned up and closed.
    pub async fn finalize(&mut self) {

        self.unregister_ads_router_notification();
        self.unregister_state_notification();
        self.unregister_all_symbols();

        info!("AdsClient closing connection to ADS Router...");
        if self.current_comms_port != 0 {
            unsafe {

                // Close the Ex function port.
                AdsPortCloseEx(self.current_comms_port);

                // Close the legacy function port.
                crate::AdsPortClose();

            }
            self.current_comms_port = 0;
        }        

        info!("AdsClient stopping notification thread...");
        self.shutdown_signal.store(true, Ordering::SeqCst);
        if let Some(jh) = self.notification_join_handle.take() {

            match jh.await {
                Ok(_) => info!("AdsClient notification thread closed cleanly."),
                Err(err) => error!("Failed to gracefully shut down notification thread: {:?}", err),
            }
        }

        info!("AdsClient clear symbol entries...");
        self.symbol_entries.clear();

        info!("AdsClient finalize complete.");
    }


    fn register_state_notification(&mut self) {
        let raw_address = &mut self.address as *mut AmsAddr;


        let changeFilter = AdsNotificationAttrib__bindgen_ty_1 {
            dwChangeFilter :0
        };

        let mut adsNotificationAttrib = AdsNotificationAttrib  {
            cbLength: 2,
            nTransMode: nAdsTransMode_ADSTRANS_SERVERONCHA,
            nMaxDelay: 0,
            __bindgen_anon_1 : changeFilter
        };

        let raw_attr = &mut adsNotificationAttrib as *mut AdsNotificationAttrib;              
        let ptr_notification_handle = &mut self.ads_state_notification_handle as *mut u32;

        unsafe {


            let state_noti_err = AdsSyncAddDeviceNotificationReqEx(
                self.current_comms_port, 
                raw_address, 
                ADSIGRP_DEVICE_DATA, 
                ADSIOFFS_DEVDATA_ADSSTATE,
                raw_attr,
                Some(ads_state_callback),
                self.id,
                ptr_notification_handle    
            );

            if state_noti_err != 0 {
                log::error!("Failed to register ADS State notification callbacks: {}", state_noti_err);
            }            
            else {
                log::info!("Registered ADS State notification callback successfully.");
            }


        }

    }




    /// Unregister the callback from the ADS router. Should be called during finalize
    /// to avoid bogging down the ADS Router. 
    fn unregister_state_notification(&mut self) {

        if self.ads_state_notification_handle == 0 {
            return;
        }

        let raw_address = &mut self.address as *mut AmsAddr;

        unsafe {


            let state_noti_err = AdsSyncDelDeviceNotificationReqEx(
                self.current_comms_port, 
                raw_address, 
                self.ads_state_notification_handle
            );

            if state_noti_err != 0 {
                log::error!("Failed to remove ADS State notification callbacks: {}", state_noti_err);
            }            
            else {
                log::info!("Removed ADS State notification callback successfully.");
            }


        }

    }


    /// Register to receive state notifications from the ADS Router, which is our pipeline to the
    /// targets.
    /// Note that notifications will only come when the 
    fn register_ads_router_notification(&self) {
        unsafe {
            let router_state_err = AdsAmsRegisterRouterNotification(Some(ads_router_callback));

            if router_state_err != 0 {
                log::error!("Failed to register ADS Router notification callbacks: {}", router_state_err);
            }            
            else {
                log::info!("Registered ADS Router notification callback successfully.");
                
            }            
        }
        

        
    }

    /// Unregester state notifications from the ADS Router to avoid bogging down the ADS router.
    fn unregister_ads_router_notification(&self) {
        unsafe {
            let err = AdsAmsUnRegisterRouterNotification();

            if err != 0 {
                log::error!("Failed to remove ADS Router notification callbacks: {}", err);
            }            
            else {
                log::info!("Removed ADS Router notification callback successfully.");
            }                 
        }
        
  
    }


    /// Upload and buffer all available symbols from the controller.
    #[allow(dead_code)]
    pub fn upload_all_symbols(&mut self) {
        if let Some(res) = ads_symbol_loader::upload_symbols(&self.address, self.current_comms_port) {
            self.symbol_collection = res;
        }
    }


    /// Unregister and reset all symbols. This will require uploading the symbols again before
    /// reading and writing.
    pub fn reset_uploaded_symbols(&mut self) {
        self.unregister_all_symbols();
        self.symbol_collection.symbols.clear();
        self.symbol_collection.data_types.clear();
    }

    /// Search through the uploaded symbol collection to find all items belonging to a particular domain. 
    /// This method always returns a vector. If no tags exist for the provided domain, an empty vector is returned.
    #[allow(dead_code)]
    pub fn find_symbols_in_domain(&self, domain : &str) -> Vec<AdsSymbolInfo> {
        return self.symbol_collection.find_symbols_in_domain(domain);
    }


    /// Return the handle for a symbol in the PLC, it it exists.
    /// Returns handle > 0 if found, 0 if not.    
    #[allow(dead_code)]
    fn get_handle_by_name(&mut self, symbol_name: &str) -> Result<u32, anyhow::Error> {
        let raw_address = &mut self.address as *mut AmsAddr;
        let mut handle : u32 = 0;
        let ptr_handle = &mut handle as *mut u32 as *mut c_void;
        let ptr_name = symbol_name as *const str as *mut c_void;
        let mut bytes_read : u32 = 0;
        let ptr_bytes_read = &mut bytes_read as *mut u32;

        unsafe {

            let err = AdsSyncReadWriteReqEx2(
                self.current_comms_port,
                raw_address, 
                ADSIGRP_SYM_HNDBYNAME, 
                0x0,
                std::mem::size_of::<u32>().try_into().unwrap(),
                ptr_handle,
                symbol_name.len() as u32,
                ptr_name,
                ptr_bytes_read
            );


            if err != 0 {

                return Err(anyhow!("Error reading handle for symbol {}: ADS error code {}", 
                    symbol_name, 
                    err
                ));
            }
            else {
                
                return Ok(handle);
            }

        }

    }

    


    /// Upload information about a symbol from the PLC
    #[allow(dead_code)]
    pub fn upload_symbol_entry(&mut self, symbol_name : &str) -> Result<AdsSymbolEntry, anyhow::Error> {

        let raw_address = &mut self.address as *mut AmsAddr;
        let mut handle : u32 = 0;
        let ptr_handle = &mut handle as *mut u32 as *mut c_void;
        let ptr_name = symbol_name as *const str as *mut c_void;
        let mut bytes_read : u32 = 0;
        let ptr_bytes_read = &mut bytes_read as *mut u32;

        let mut symbol_entry = AdsSymbolEntry {
            entryLength: 0,
            iGroup: 0,
            iOffs: 0,
            size: 0,
            dataType: 0,
            flags: 0,
            nameLength: 0,
            typeLength: 0,
            commentLength: 0,
        };

        let ptr_symbol_entry = &mut symbol_entry as *mut AdsSymbolEntry as *mut c_void;

        unsafe {

            // get the handle of the named symbol

            let err = AdsSyncReadWriteReqEx2(
                self.current_comms_port,
                raw_address, 
                ADSIGRP_SYM_HNDBYNAME, 
                0x0,
                std::mem::size_of::<u32>().try_into().unwrap(),
                ptr_handle,
                symbol_name.len() as u32,
                ptr_name,
                ptr_bytes_read
            );


            if err != 0 {
                return Err(anyhow!("Error reading symbol: ADS error code {}", err));
            }
            else {


                let info_err = AdsSyncReadWriteReqEx2(
                    self.current_comms_port,
                    raw_address, 
                    ADSIGRP_SYM_INFOBYNAMEEX,
                    0,
                    std::mem::size_of::<AdsSymbolEntry>() as u32,
                    ptr_symbol_entry,
                    symbol_name.len() as u32,
                    ptr_name,
                    std::ptr::null_mut()
                );


                if info_err == 0 {                    
                    return Ok(symbol_entry);
                }
                else {

                    return Err(anyhow!("Error reading symbol: ADS error code {}", err));
                }
            
            }

        }


    }

    
    /// Returns the entry information for a symbol if buffered locally. If not buffered locally,
    /// the symbol information is uploaded and, if upload is successful, buffered for the next use.
    /// 
    /// The AdsSymbolEntry describes the data location and type of a symbol in the target PLC. This information
    /// is needed for synchronouse writes and reads.
    /// 
    /// It will generally be more peformant to call this function instead of upload_symbol_info, unless you need
    /// to ensure that the symbol info is uploaded from the PLC.
    /// 
    /// Buffer entry information is cleared any time the connection is lost.
    pub fn get_symbol_entry(&mut self, symbol_name : &str) -> Result<AdsSymbolEntry, anyhow::Error> {

        if self.symbol_entries.contains_key(symbol_name) {
            return Ok(self.symbol_entries[symbol_name]);
        }
        else {
            return self.upload_symbol_entry(symbol_name);
        }

    }


    /// Synchronous write of a value to the target PLC as a stream of bytes.
    /// This is the primaruy function for writing any value to the PLC. 
    fn write_raw_bytes(&mut self, symbol_name: &str, bytes : &[u8] ) -> Result<(),anyhow::Error> {

        let symbol_information = self.get_symbol_entry(symbol_name);
        if let Ok(info) = symbol_information{

            let raw_address = &mut self.address as *mut AmsAddr;

            // AdsSyncWriteReqEx requires a mutable pointer to the byte buffer, so we need to 
            // make a copy, then a pointer to that copy.
            let mut cloned_buffer = bytes.to_vec();
            let ptr_buffer = cloned_buffer.as_mut_ptr() as *mut c_void;

            unsafe {
                let rc = AdsSyncWriteReqEx(
                    self.current_comms_port,
                    raw_address,
                    info.iGroup,
                    info.iOffs,
                    bytes.len() as u32,
                    ptr_buffer
                );
            
                if rc != 0
                {
                    Err(anyhow!("Failed to write symbol {} with error {}", symbol_name, rc))
                }                
                else {
                    Ok(())
                }
            }

        }
        else {
            Err(anyhow!("Failed to read information for symbol {}", symbol_name))
        }

    }
    

    /// Synchrnous read of a value in the target PLC, returned as a stream of bytes.
    /// This is the base function for reading any value from the PLC. 
    fn read_raw_bytes(&mut self, symbol_name: & str) -> Result<Vec<u8>, anyhow::Error> {
  
        let raw_address = &mut self.address as *mut AmsAddr;
        // The "handle" or id within the controller that identifies a symbol.
        let mut handle : u32 = 0;
        // A void pointer to that handle we can pass to C functionc calls expecting a void*
        let ptr_handle = &mut handle as *mut u32 as *mut c_void;
        let ptr_name = symbol_name as *const str as *mut c_void;
        let mut bytes_read : u32 = 0;
        let ptr_bytes_read = &mut bytes_read as *mut u32;



        let symbol_information = self.get_symbol_entry(symbol_name);

        if let Ok(info) = symbol_information{

            unsafe {

                // get the handle of the named symbol
    
                let err = AdsSyncReadWriteReqEx2(
                    self.current_comms_port,
                    raw_address, 
                    ADSIGRP_SYM_HNDBYNAME, 
                    0x0,
                    std::mem::size_of::<u32>().try_into().unwrap(),
                    ptr_handle,
                    symbol_name.len() as u32,
                    ptr_name,
                    ptr_bytes_read
                );
    
    
                if err != 0 {
                    // an error occurred.
    
                    return Err(anyhow!("Error {} occurred!", err));
                }
                else {

                    // Create a buffer of the appropriate sise, filled with 0's.
                    let mut buffer = vec![0u8;info.size as usize];
                    // Create a pointer to that under
                    let ptr_buffer = buffer.as_mut_ptr() as *mut c_void;

    
                    let read_err = AdsSyncReadReqEx2( 
                        self.current_comms_port,                   
                        raw_address, 
                        ADSIGRP_SYM_VALBYHND, 
                        handle,
                        info.size,
                        ptr_buffer,
                        std::ptr::null_mut()
                    );

                    if read_err != 0 {
                        // TODO: convert error code to error string.
                        return Err(anyhow!("Error {} occurred reading value into buffer", read_err));
                    }
                    else {
                        return Ok(buffer);
                    }
                
                }
            
            }            


        }
        else {
            // TODO: convert error code to error string.
            return Err(anyhow!("Error : could not obtain handle to symbol. Symbol exist?"));
        }

  
    }


    /// Synchronous read of a symbol based upon its name in the PLC.
    /// Complete, fully-qualified name required.
    pub fn read_symbol_by_name(&mut self, symbol_name : &str) -> Result<variant::VariantValue, anyhow::Error>  {
        match self.read_raw_bytes(symbol_name) {
            Ok(buffer) => {
                let symbol_information = self.get_symbol_entry(symbol_name);
                match symbol_information {
                    Ok(info) => {
                        match AdsDataTypeId::try_from(info.dataType) {
                            Ok(dt) => {
                                if let Some(symbol_info) = self.symbol_collection.symbols.get(symbol_name) {
                                    if let Some(ret) = ads_data::deserialize(
                                        &self.symbol_collection,
                                        symbol_info,
                                        &buffer, 
                                        &dt
                                    ) {
                                        return Ok(ret);
                                    }
                                    else {
                                        return Err(anyhow!("Failed to read symbol: {}", symbol_name));
                                    }        
                                }
                                else {
                                    return Err(anyhow!("Failed to find symbol {} in uploaded symbol table.", symbol_name));
                                }
                            },
                            Err(err) => {
                                return Err(anyhow!("Unsupported type {} for symbol: {}", err, symbol_name));
                            }    
                        }
                    },
                    Err(err) => Err(anyhow!("{}",err))
                }
            },
            Err(err) => Err(err),
        }

    }

    /// Read a plain-old-data value from the PLC and structures that contain only plain-old data. Does
    /// not support dynamically-allocated types, like String and Vec. 
    /// 
    /// Fixed strings are possible:
    /// ```
    /// fixed_str: [u8; 32],
    /// ```
    pub fn read_symbol_value<T: FromBytes + Default>(&mut self, symbol_name: &str) -> Result<T, anyhow::Error> 
    where
        T: Default + AsBytes + FromBytes, // ZeroCopy constraints
    {
        match self.read_raw_bytes(symbol_name) {
            Ok(buffer) => {
                // Ensure buffer is large enough to contain T
                if buffer.len() < std::mem::size_of::<T>() {

                    return Err(anyhow!(
                        "Buffer size {} is too small for type size {}", 
                        buffer.len(), 
                        std::mem::size_of::<T>()
                    ));
                }
            
                // Use ZeroCopy to convert the buffer into T
                match T::read_from(&*buffer) {
                    Some(value) => Ok(value),
                    None => Err(anyhow!("Failed to convert buffer to type. Buffer size: {}", buffer.len()))
                }
            
            },
            Err(err) => return Err(anyhow!("{}",err))
        }

    }    



    /// Read a symbol value specifically as a string.
    pub fn read_symbol_string_value(&mut self, symbol_name: &str) -> Result<String, anyhow::Error> 
    {
        match self.read_raw_bytes(symbol_name) {
            Ok(buffer) => {                
                return vec_to_string(&buffer);
            },
            Err(err) => return Err(anyhow!("{}",err))
        }

    }   



    /// Write the value of the specified type to the target. The type is serialized and transmitted.
    /// Dynamic types (like Vec or String) are not supported.
    pub fn write_symbol_value<T: AsBytes>(&mut self, symbol_name: &str, value: T) -> Result<() , anyhow::Error>
    where
        T: AsBytes, // ZeroCopy constraint for writing
    {
        let bytes = value.as_bytes();
    
        // Here, you would write the logic to send `bytes` to the PLC.
        // This could involve sending the bytes over a network, writing to a file, etc.
        // For this example, let's assume a function `write_raw_bytes` does this job.
        return self.write_raw_bytes(symbol_name, bytes);
    }    


    /// Write a string slice to the target. The string is converted to bytes and transmitted.
    /// Note that the typical string in the PLC will be using 8-bit ASCII text.
    pub fn write_symbol_string_value(&mut self, symbol_name: &str, value: &str) -> Result<(), anyhow::Error>
    {
        let bytes = value.as_bytes();
    
        // Here, you would write the logic to send `bytes` to the PLC.
        // This could involve sending the bytes over a network, writing to a file, etc.
        // For this example, let's assume a function `write_raw_bytes` does this job.
        return self.write_raw_bytes(symbol_name, bytes);
    }  


    /// Write a symbol with the value contained in the Variant. The underlying value of VariantValue is
    /// expected to match the target symbol in the PLC exactly; this function will not convert.
    pub fn write_symbol_variant_value(&mut self, symbol_name: &str, value: &VariantValue) -> Result<(), anyhow::Error> {

        

        if let Some(symbol_info) = self.symbol_collection.symbols.get(symbol_name) {           

            match serialize(&self.symbol_collection, &symbol_info, value) {
                Ok(bytes) => {

                    //log::debug!("\n{:?}\n", bytes);
                    return self.write_raw_bytes(symbol_name, &bytes);
                },
                Err(err) => {
                    return Err(anyhow!("Failed to serialize value: {}", err));
                },
            }
        }
        else {
            return Err(anyhow!("Failed to find information on symbol {}", symbol_name));
        }

    }



    /// Register a symbol for on-data-change notification.
    /// Returns true if successful, false if not.
    pub fn register_symbol(&mut self, symbol_name : &str) -> Result<(), anyhow::Error> {

        if  self.symbol_collection.symbols.contains_key(symbol_name) {

            let symbol = &self.symbol_collection.symbols[symbol_name];

            let symbol_data_type = AdsDataTypeId::try_from(symbol.type_id);

            if let Err(_) = symbol_data_type {
                return Err(anyhow!("Unsupported data type id: {}", symbol.type_id));
            }

            //
            // Bindgen somehow flubbed the generation of the nCycleTime member, and turned it into
            // a union.
            //
            let nCycleTime = AdsNotificationAttrib__bindgen_ty_1 {nCycleTime: 100000}; // unit = 100ns, 10msec = 100000

            let mut attrib = AdsNotificationAttrib {
                cbLength : symbol.size,
                nTransMode : nAdsTransMode_ADSTRANS_SERVERONCHA,
                nMaxDelay: 0,
                __bindgen_anon_1 : nCycleTime       // nCycleTimeMember
            };
            
            
            match self.get_handle_by_name(symbol_name) {

                Ok(handle) => { 

                    let raw_address = &mut self.address as *mut AmsAddr;
                    let mut notification_handle : u32 = 0;
                    let ptr_notification_handle = &mut notification_handle as *mut u32;
                    let ptr_attrib = &mut attrib as *mut AdsNotificationAttrib;

                    unsafe {

                        //
                        // A note about callbacks:
                        // Callback signatures typically get generated as Option<unsafe extern "C" fn ...> 
                        // in bindgen. So when you write your callbacks in Rust, you obviously need to decorate 
                        // them with unsafe extern "C" (they don’t need to be pub), and then when you pass them 
                        // to the C library you just wrap the name in Some.
                        //

                        info!("Registering notification for {} with handle {}", symbol_name, handle );
                        
                        let notitication_err = AdsSyncAddDeviceNotificationReqEx(
                            self.current_comms_port,
                            raw_address,
                            ADSIGRP_SYM_VALBYHND,
                            handle,
                            ptr_attrib,
                            Some(ads_value_callback),   // see note above
                            handle,
                            ptr_notification_handle
                        );

                        if notitication_err != 0 {
                            return Err(anyhow!("Failed to register handle: {}", notitication_err));
                        }

                    }


                    add_registered_symbol(RegisteredSymbol { 
                        handle: handle, 
                        notification_handle: notification_handle, 
                        name: symbol_name.to_string(),
                        data_type_id: symbol_data_type.unwrap()
                    });

                    return Ok(());
                },
                Err(err) => {
                    return Err(err);
                }
            }

        }
        else {
            return Err(anyhow!("The requested symbol {} does not exist in the controller. 
                Please check symbol name and connection properties.",
                symbol_name
            ));
        }

    }


    /// Unregister a symbol that was registered for notification. It's important to unregister
    /// a notification before closing connections, or the ADS router will get bogged down.
    pub fn unregister_symbol(&mut self, symbol_name : &str ) -> Result<(), anyhow::Error> {

        let raw_address = &mut self.address as *mut AmsAddr;
        match get_registered_symbol_by_name(symbol_name) {
            Ok(sym) => {
                unsafe{
                    let notitication_err = AdsSyncDelDeviceNotificationReqEx(
                        self.current_comms_port,
                        raw_address,    
                        sym.notification_handle
                    );

                    if notitication_err != 0 {
                        return Err(anyhow!("Failed to remove notification: {}", notitication_err));
                    }
                    else {
                        info!("Removed notification for {}", symbol_name);

                        if let Err(err) = remove_registered_symbol_by_name(symbol_name) {
                            error!("Failed to remove notification symbol from local registry: {}", err);
                        }

                        return Ok(());
                    }
                }
        
            },
            Err(err) => return Err(err)
        }

    }


    /// Unregister notifications for all symbols registered with the ADS Router.
    /// This function should be called before closing the connection to the ADS Router. 
    pub fn unregister_all_symbols(&mut self) {

        

        while let Some(item) = get_last_registered_symbol() {

            if let Err(err) = self.unregister_symbol(&item.name) {
                log::error!("Failed to unregister symbol {} : {}", item.name, err);
            }
            else {
                log::info!("Unregistered symbol {}", item.name);
            }    
            
        }
    }


}


// -----------------------------------------
// -----------------------------------------
//
// Callbacks from the C API
//
// -----------------------------------------
// -----------------------------------------




/// Callback from the 32-bit Beckhoff Ads DLL signalling a data-change event
/// for a symbol. The request is forwarded to handle_ads_value_callback.
/// 
/// # Arguments
/// * `pAddr` - The AMS address of the controller sending the data.
/// * `pNotification` - A structure containing the event data.
/// * `hUser` - An id for the adsclient instance that registered this callback. 
/// 
#[cfg(target_arch = "x86")]
unsafe extern "stdcall" fn ads_value_callback(
    pAddr: *mut AmsAddr,
    pNotification: *mut AdsNotificationHeader,
    hUser: ::std::os::raw::c_ulong
) {
    handle_ads_value_callback(pAddr, pNotification, hUser);
}


/// Callback from the 64-bit Beckhoff Ads DLL signalling a data-change event
/// for a symbol. The request is forwarded to handle_ads_value_callback.
/// 
/// # Arguments
/// * `pAddr` - The AMS address of the controller sending the data.
/// * `pNotification` - A structure containing the event data.
/// * `hUser` - An id for the adsclient instance that registered this callback. 
/// 
#[cfg(target_arch = "x86_64")]
unsafe extern "C" fn ads_value_callback(
    pAddr: *mut AmsAddr,
    pNotification: *mut AdsNotificationHeader,
    hUser: ::std::os::raw::c_ulong
) {
    handle_ads_value_callback(pAddr, pNotification, hUser);
}

/// Does the actual work of handing the value callback for both
/// 32-bit and 64-bit systems.
/// 
/// Callback from the Beckhoff Ads DLL signalling a data-change event
/// for a symbol.
/// 
/// # Arguments
/// * `pAddr` - The AMS address of the controller sending the data.
/// * `pNotification` - A structure containing the event data.
/// * `hUser` - An id for the adsclient instance that registered this callback. 
/// 
/// # Remarks
/// 
/// - You have to be careful with data races in this callback. The ADS router
/// can, and often will, send out the initial value for the symbol 
/// before sending back the notification handle to the method that registered
/// the on-data-change callback.
/// 
/// - Callback signatures typically get generated as Option<unsafe extern "C" fn ...> 
/// in bindgen. So when you write your callbacks in Rust, you obviously need to decorate 
/// them with unsafe extern "C" (they don’t need to be pub), and then when you pass them 
/// to the C library you just wrap the name in Some.
///
/// 
/// 
/// ## AdsNotificationHeader
/// ```
///    pub struct AdsNotificationHeader {
///        pub hNotification: ::std::os::raw::c_ulong,
///        pub nTimeStamp: ::std::os::raw::c_longlong,
///        pub cbSampleSize: ::std::os::raw::c_ulong,
///        pub data: [::std::os::raw::c_uchar; 1usize],
///    }
/// ``` 
/// 
/// 
/// 
unsafe fn handle_ads_value_callback(
    _pAddr: *mut AmsAddr,
    pNotification: *mut AdsNotificationHeader,
    hUser: ::std::os::raw::c_ulong
) {

    let id = hUser as u32;

    if let Ok(_) = get_registered_symbol_by_handle(id) {

        // Assuming `cbSampleSize` gives the size of the data in bytes
        let data_size = (*pNotification).cbSampleSize as usize;

        // Create a slice from the `data` pointer for safe access
        // This is safe because we trust `cbSampleSize` to give us the correct length of the data
        let data_slice = slice::from_raw_parts((*pNotification).data.as_ptr(), data_size);        
                
        let global_sender = SYMBOL_MANAGER.sender.lock().unwrap();

        for i in 0 .. global_sender.len() {

            let dcei = RouterNotificationEventInfo {
                event_type : EventInfoType::DataChange,
                id : id,
                data : Vec::from(data_slice)
            };

            match global_sender[i].try_send(dcei) {
                Ok(_) => {},
                Err(err) => log::error!("Failed to send on notification channel for DATA CHANGE: {}", err),
            }
                
        }

    }
    else {
        log::warn!(
            "CALLBACK: Unknown handle {} received for on value change notification.",
            id
        );
    }

   

}





/// Callback from the 32-bit Beckhoff Ads DLL signalling a change in the target state.
/// Usually, this means the PLC changed its run state.
/// 
/// # Arguments
/// * `pAddr` - The AMS address of the controller sending the data.
/// * `pNotification` - A structure containing the event data.
/// * `hUser` - An id for the adsclient instance that registered this callback. 
/// 
#[cfg(target_arch = "x86")]
unsafe extern "stdcall" fn ads_state_callback(
    pAddr: *mut AmsAddr,
    pNotification: *mut AdsNotificationHeader,
    hUser: ::std::os::raw::c_ulong
) {
    handle_ads_state_callback(pAddr, pNotification, hUser);
}


/// Callback from the 32-bit Beckhoff Ads DLL signalling a change in the target state.
/// Usually, this means the PLC changed its run state.
/// 
/// # Arguments
/// * `pAddr` - The AMS address of the controller sending the data.
/// * `pNotification` - A structure containing the event data.
/// * `hUser` - An id for the adsclient instance that registered this callback. 
/// 
#[cfg(target_arch = "x86_64")]
unsafe extern "C" fn ads_state_callback(
    pAddr: *mut AmsAddr,
    pNotification: *mut AdsNotificationHeader,
    hUser: ::std::os::raw::c_ulong
) {
    handle_ads_state_callback(pAddr, pNotification, hUser);
}


/// Callback when the state of the ADS Router has changed. This is a global event, as
/// the Router is the hub connecting us to all targets, not just the particular client instance.
unsafe fn handle_ads_state_callback(
    _pAddr: *mut AmsAddr,
    pNotification: *mut AdsNotificationHeader,
    hUser: ::std::os::raw::c_ulong
) {
    let id = hUser as u32;
    
    let global_sender = SYMBOL_MANAGER.sender.lock().unwrap();

    // Assuming `cbSampleSize` gives the size of the data in bytes
    let data_size = (*pNotification).cbSampleSize as usize;

    // Create a slice from the `data` pointer for safe access
    // This is safe because we trust `cbSampleSize` to give us the correct length of the data
    let data_slice = slice::from_raw_parts((*pNotification).data.as_ptr(), data_size);    

    if let Some(state ) = i16::read_from(data_slice) {

        for i in 0 .. global_sender.len() {

            let dcei = RouterNotificationEventInfo {
                event_type : EventInfoType::AdsState,
                id : id,
                data : state.to_le_bytes().to_vec()
            };
    
            match global_sender[i].try_send(dcei) {
                Ok(_) => {},
                Err(err) => log::error!("Failed to send on CHANNEL for ADS STATE CHANGE: {}", err),
            }
                
        }

    }




}



/// Callback from the 32-bit Beckhoff Ads DLL signalling a change in the ADS Router state.
/// Usually, this means the PLC changed its run state.
/// 
/// # Arguments
/// * `state` - Updated state of the ADS router
/// 
#[cfg(target_arch = "x86")]
unsafe extern "stdcall" fn ads_router_callback(
    state: ::std::os::raw::c_long
) {
    handle_ads_router_callback(pAddr, pNotification, hUser);
}


/// Callback from the 32-bit Beckhoff Ads DLL signalling a change in the target state.
/// Usually, this means the PLC changed its run state.
/// 
/// # Arguments
/// * `state` - Updated state of the ADS router

/// 
#[cfg(target_arch = "x86_64")]
unsafe extern "C" fn ads_router_callback(
    state: ::std::os::raw::c_long
) {
    handle_ads_router_callback(state);
}




/// Handles the callback from the ADS router that it has changed state.
/// Note that the callback will only be issued when the state changes, and there
/// will be no initial callback when the connection is made, unlike other notifications.
unsafe fn handle_ads_router_callback(
    state: ::std::os::raw::c_long
) {
    
    let global_sender = SYMBOL_MANAGER.sender.lock().unwrap();
    
    for i in 0 .. global_sender.len() {

        let dcei = RouterNotificationEventInfo {
            event_type : EventInfoType::RouterState,
            id : 32767, // BROADCAST
            data : state.to_le_bytes().to_vec()
        };


        match global_sender[i].try_send(dcei) {
            Ok(_) => {},
            Err(err) => log::error!("Failed to send on CHANNEL for ADS ROUTER CHANGE: {}", err),
        }
            
    }


}





// -----------------------------------------
// -----------------------------------------
//
// Module scope functions
//
// -----------------------------------------
// -----------------------------------------



/// Get the current version of the TcAdsDll
pub fn get_dll_version() -> i32 {
    unsafe {
        let val : i32 = AdsGetDllVersion();
        info!("The ADS Dll version is {}", val);
        return val;    
    }    
}


/// Returns the AMS address of the local TwinCAT system.
pub fn get_local_address() -> AmsAddr {
    let mut local_addr = AmsAddr::new();

    unsafe {
        AdsGetLocalAddress(&mut local_addr);
    }

    return local_addr;
}



/// Convert a string to an AmsAddr structure to be used with the
/// TwinCAT router. Returns None if the address is at all invalid.
pub fn to_ams_addr(s :&str) -> Option<AmsAddr> {


    //
    // First, split the address into tokens separated by the . character
    let tokens = s.trim().split_terminator('.')
        .collect::<Vec<_>>();

    if tokens.len() != 6 {
        log::warn!("{} does not contain 6 tokens. Num tokens: {}", s, tokens.len());
        return None;
    }

    let mut ret = AmsAddr::new();

    for i in 0..6 {

        match u8::from_str(tokens[i]) {
            Ok(b) => ret.netId.b[i] = b,
            Err(err) => {
                log::warn!("Error parsing {}: {}", tokens[i], err);
                return None;
            }
        }
    } 
    

    return Some(ret);

}






#[test]
fn hello_ads() {
    println!("Hello, ADS!");

    unsafe {
        let val = AdsGetDllVersion();

        println!("The ADS Dll version is {}", val);

        let client_port = crate::AdsPortOpen();

        println!("The ADS client port is {}", client_port);


        let mut local_addr = AmsAddr::new();

        AdsGetLocalAddress(&mut local_addr);

        println!("local addr is {}.{}.{}.{}.{}.{}", 
            local_addr.netId.b[0], 
            local_addr.netId.b[1], 
            local_addr.netId.b[2], 
            local_addr.netId.b[3], 
            local_addr.netId.b[4], 
            local_addr.netId.b[5]
        );

        std::thread::sleep(std::time::Duration::from_millis(1000));


        crate::AdsPortClose();

        println!("Goodbye...");

    }
}  




