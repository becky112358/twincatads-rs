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

use indexmap::IndexMap;
use lazy_static::lazy_static;
use log::{error, info};
use std::collections::HashMap;
use std::os::raw::c_void; // c_void, etc
use std::slice;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::task::JoinHandle;

use anyhow::anyhow;
use zerocopy::{AsBytes, FromBytes};

use mechutil::notifier::Notifier;
use mechutil::variant::{self, VariantValue};

use crate::client::ads_symbol_loader;
use crate::client::client_types::EventInfoType;
use crate::nAdsTransMode_ADSTRANS_SERVERONCHA;
use crate::AdsGetDllVersion;
use crate::AdsGetLocalAddress;
use crate::AdsNotificationAttrib;
use crate::AdsNotificationAttrib__bindgen_ty_1;
use crate::AdsNotificationHeader;
use crate::AdsPortCloseEx;
use crate::AdsPortOpenEx;
use crate::AdsSymbolEntry;
use crate::AdsSyncAddDeviceNotificationReqEx;
use crate::AdsSyncDelDeviceNotificationReqEx;
use crate::AdsSyncReadReqEx2;
use crate::AdsSyncReadWriteReqEx2;
use crate::AdsSyncWriteReqEx;
use crate::ADSIGRP_DEVICE_DATA;
use crate::ADSIGRP_SYM_HNDBYNAME;
use crate::ADSIGRP_SYM_INFOBYNAMEEX;
use crate::ADSIGRP_SYM_VALBYHND;
use crate::ADSIOFFS_DEVDATA_ADSSTATE;
use crate::{
    AdsAmsRegisterRouterNotification, AdsAmsUnRegisterRouterNotification, AdsPortClose,
    AdsSyncAddDeviceNotificationReq, AdsSyncDelDeviceNotificationReq, AmsAddr, ADSIGRP_SYM_VERSION,
};

use super::ads_symbol_loader::{AdsDataTypeInfo, AdsSymbolCollection, AdsSymbolInfo};
use super::client_types::{AdsClientNotification, RegisteredSymbol, RouterNotificationEventInfo};

use super::ads_data::{self, serialize, vec_to_string, AdsDataTypeId};

#[allow(dead_code)]
struct SymbolManager {
    /// Symbols that have been registered for on-data noticifcation.
    /// The key is the handle (not the notification handle) for the symbol
    /// that was collected in the first step of registering the symbol.
    pub registered_symbols: Mutex<Vec<RegisteredSymbol>>,
    pub notify: Mutex<Notifier<'static, RouterNotificationEventInfo>>,
    //pub data_change : Mutex<DataChangeCallback>,
    pub sender: Mutex<Vec<Sender<RouterNotificationEventInfo>>>,
    pub receiver: Mutex<Option<Receiver<RouterNotificationEventInfo>>>,
    pub is_running: Mutex<bool>,
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
fn get_registered_symbol_by_name(symbol_name: &str) -> Result<RegisteredSymbol, anyhow::Error> {
    let guard = SYMBOL_MANAGER.registered_symbols.lock().unwrap();

    for i in 0..guard.len() {
        let item = &guard[i];

        if item.name == symbol_name {
            return Ok(item.clone());
        }
    }

    Err(anyhow!(
        "Symbol name {} is not registered in the notification collection.",
        symbol_name
    ))
}

/// Remove a symbol registered for on value change notification from the list.
fn remove_registered_symbol_by_name(symbol_name: &str) -> Result<(), anyhow::Error> {
    let mut guard = SYMBOL_MANAGER.registered_symbols.lock().unwrap();

    for i in 0..guard.len() {
        let item = &guard[i];

        if item.name == symbol_name {
            guard.remove(i);
            return Ok(());
        }
    }

    Err(anyhow!(
        "Failed to locate symbol name {} to remove.",
        symbol_name
    ))
}

/// Finds and returns the symbol registered for on value change notification from the global list.
fn get_registered_symbol_by_handle(handle: u32) -> Result<RegisteredSymbol, anyhow::Error> {
    let guard = SYMBOL_MANAGER.registered_symbols.lock().unwrap();

    for i in 0..guard.len() {
        let item = &guard[i];

        if item.handle == handle {
            return Ok(item.clone());
        }
    }

    Err(anyhow!(
        "Symbol handle {} is not registered in the notification collection.",
        handle
    ))
}

/// Get the last symbol stored in the collection, if it exists.
fn get_last_registered_symbol() -> Option<RegisteredSymbol> {
    let guard = SYMBOL_MANAGER.registered_symbols.lock().unwrap();

    if guard.len() > 0 {
        guard.last().cloned()
    } else {
        None
    }
}

/// Add a symbol to the list of registered symbols used when on value changed notifications are received.
fn add_registered_symbol(s: RegisteredSymbol) {
    let mut mutex = SYMBOL_MANAGER.registered_symbols.lock().unwrap();

    info!(
        "Adding register symbol {} with handle {} ",
        s.name, s.handle
    );
    mutex.push(s);
}

/// Returns an id to identify the AdsClient instance. The same
/// ID is never use twice in the same run-time.
fn get_new_id() -> u32 {
    static COUNTER: AtomicU32 = AtomicU32::new(1);
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
    id: u32,

    /// target address of the remote PLC.
    address: AmsAddr, // netId, port

    /// The communications port to the ADS router. This is
    /// not the same as a connection to the PLC, but is a
    /// required first step.
    current_comms_port: i32,

    symbol_collection: AdsSymbolCollection,

    /// A buffer of symbol entries uploaded from the PLC. Symbol Entry information
    /// is used for synchronous reads and writes. This map must be cleared when the
    /// connection is lost, when the PLC is re-started or when an the project is re-actviated.
    symbol_entries: HashMap<String, AdsSymbolEntry>,

    /// Notification channel back to the parent that owns this instance of the AdsClient.
    notification_tx: mpsc::Sender<AdsClientNotification>,

    /// Stores the join handle for the notification thread.
    notification_join_handle: Option<JoinHandle<()>>,

    /// Signal used to shut down the message loop that handles notifications between
    /// threads.
    shutdown_signal: Arc<AtomicBool>,

    /// Notification handle for the ADS State callback.
    ads_state_notification_handle: u32,

    /// Notification handle detecting changes in the target device symbol table.
    ads_symbol_table_notification_handle: u32,

    /// Re-registration count. Just connecting will give us a change signal, so
    /// we count to see if we should actually notify the parent that a change occurred.
    symbol_reregistration_count: usize,
}

impl AdsClient {
    /// Constructor
    pub fn new() -> (Self, mpsc::Receiver<AdsClientNotification>) {
        let (tx, rx) = mpsc::channel(100);

        let client = AdsClient {
            id: get_new_id(),
            address: get_local_address(),
            current_comms_port: 0,
            symbol_collection: AdsSymbolCollection::default(),
            symbol_entries: HashMap::new(),
            notification_tx: tx,
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            notification_join_handle: None,
            ads_state_notification_handle: 0,
            ads_symbol_table_notification_handle: 0,
            symbol_reregistration_count: 0,
        };

        (client, rx)
    }

    /// Set the target address of the device.
    pub fn set_address(&mut self, s: &str) {
        if !s.is_empty() {
            if let Some(addr) = to_ams_addr(s) {
                self.address = addr;
            } else {
                log::warn!("Could not parse address as provided: {}", s);
                self.address = get_local_address();
                log::warn!("Defaulting to local address.");
            }
        } else {
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
    pub fn set_port(&mut self, s: u16) {
        self.address.port = s;
    }

    /// Spin up the Ads library, open a port to the remote device, and upload the information
    /// from the remote target device.
    ///
    /// ## Returns:
    /// - Ok if successful. The connection is ready to use.
    /// - Err if fails to make connection to target device. The connection is completely closed and initialize needs to be called again.
    pub fn initialize(&mut self) -> Result<(), anyhow::Error> {
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

        let (tx, mut rx): (
            Sender<RouterNotificationEventInfo>,
            Receiver<RouterNotificationEventInfo>,
        ) = mpsc::channel(100);

        let mut global_sender = SYMBOL_MANAGER.sender.lock().unwrap();
        global_sender.push(tx);

        //
        // Buffer all the symbol information from the PLC.
        //
        match self.upload_all_symbols() {
            Ok(_) => {
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
                            info!(
                                "AdsClient: shutdown signal received, exiting notification thread."
                            );
                            break;
                        }

                        let result = { tokio::time::timeout(timeout, rx.recv()).await };

                        match result {
                            Ok(opt) => {
                                match opt {
                                    Some(info) => {
                                        match info.event_type {
                                            EventInfoType::DataChange => {
                                                if let Ok(symbol) =
                                                    get_registered_symbol_by_handle(info.id)
                                                {
                                                    // log::debug!("CHANNEL DATA NOTIFICATION: {} event received for symbol: {}",
                                                    //     info.id,
                                                    //     symbol.name
                                                    // );

                                                    // if let Some(symbol_info) = symbol_collection.symbols.get(&symbol.name) {
                                                    if let Ok(symbol_info) = symbol_collection
                                                        .get_symbol_info(&symbol.name)
                                                    {
                                                        match ads_data::deserialize(
                                                            &symbol_collection,
                                                            &symbol_info,
                                                            &info.data,
                                                            &symbol.data_type_id,
                                                        ) {
                                                            Some(var) => {
                                                                let notification = AdsClientNotification::new_datachange(&symbol.name, &var);

                                                                if let Err(err) = notification_tx
                                                                    .send(notification)
                                                                    .await
                                                                {
                                                                    error!("Failed to send notification on {} to parent with error: {}",
                                                                        symbol.name,
                                                                        err
                                                                    );
                                                                }
                                                            }
                                                            None => {
                                                                log::error!("Failed to create Variant for notification on symbol {}", symbol.name);
                                                            }
                                                        }
                                                    } else {
                                                        log::error!("Failed to locate symbol name {} in uploaded symbol table.", symbol.name);
                                                    }
                                                } else {
                                                    log::error!("Channel data notification received for unknown error.");
                                                }
                                            }
                                            EventInfoType::Invalid => {
                                                log::warn!(
                                                    "Invalid notification received. Ignoring."
                                                );
                                            }
                                            EventInfoType::AdsState => {
                                                if let Some(state) = i16::read_from(&info.data) {
                                                    let notification =
                                                        AdsClientNotification::new_ads_state(state);

                                                    if let Err(err) =
                                                        notification_tx.send(notification).await
                                                    {
                                                        error!("Failed to send ads state notification to parent with error: {}", 
                                                            err
                                                        );
                                                    }

                                                    log::info!("PLC state is now: {}", state);
                                                } else {
                                                    log::error!("Failed to extract PLC state from notification.");
                                                }
                                            }
                                            EventInfoType::RouterState => {
                                                if let Some(state) = i16::read_from(&info.data) {
                                                    let notification =
                                                        AdsClientNotification::new_router_state(
                                                            state,
                                                        );

                                                    if let Err(err) =
                                                        notification_tx.send(notification).await
                                                    {
                                                        error!("Failed to send router state notification to parent with error: {}", 
                                                            err
                                                        );
                                                    }

                                                    log::info!("ADS Router is now: {}", state);
                                                } else {
                                                    log::error!("Failed to extract PLC state from notification.");
                                                }
                                            }
                                            EventInfoType::SymbolTableChange => {
                                                let notification =
                                                    AdsClientNotification::new_symbol_table();

                                                if let Err(err) =
                                                    notification_tx.send(notification).await
                                                {
                                                    error!("Failed to send symbol table notification to parent with error: {}", 
                                                        err
                                                    );
                                                }

                                                log::info!(
                                                    "Target Device Symbol Table has changed!"
                                                );
                                            }
                                        }
                                        let _ = tokio::time::sleep(ten_millis).await;
                                    }
                                    None => {
                                        info!("Notification Sender has disconnected or faulted. No more notifications will be received.");
                                        break;
                                    }
                                };
                            }
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
                self.register_symbol_table_notification();

                Ok(())
            }
            Err(err) => {
                // This communication channel will no longer function.
                global_sender.pop();

                unsafe {
                    AdsPortCloseEx(self.current_comms_port);
                    AdsPortClose();
                }

                Err(anyhow!("{}", err))
            }
        }
    }

    /// Complete the shutdown and closing the port to the
    /// ads router. This should be done last, after all
    /// connections are cleaned up and closed.
    pub async fn finalize(&mut self) {
        self.unregister_ads_router_notification();
        self.unregister_state_notification();
        self.unregister_all_symbols();
        self.unregister_symbol_table_notification();

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
                Err(err) => error!(
                    "Failed to gracefully shut down notification thread: {:?}",
                    err
                ),
            }
        }

        info!("AdsClient clear symbol entries...");
        self.symbol_entries.clear();

        info!("AdsClient finalize complete.");
    }

    fn register_state_notification(&mut self) {
        let raw_address = &mut self.address as *mut AmsAddr;

        let changeFilter = AdsNotificationAttrib__bindgen_ty_1 { dwChangeFilter: 0 };

        let mut adsNotificationAttrib = AdsNotificationAttrib {
            cbLength: 2,
            nTransMode: nAdsTransMode_ADSTRANS_SERVERONCHA,
            nMaxDelay: 0,
            __bindgen_anon_1: changeFilter,
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
                ptr_notification_handle,
            );

            if state_noti_err != 0 {
                log::error!(
                    "Failed to register ADS State notification callbacks: {}",
                    state_noti_err
                );
            } else {
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
                self.ads_state_notification_handle,
            );

            if state_noti_err != 0 {
                log::error!(
                    "Failed to remove ADS State notification callbacks: {}",
                    state_noti_err
                );
            } else {
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
                log::error!(
                    "Failed to register ADS Router notification callbacks: {}",
                    router_state_err
                );
            } else {
                log::info!("Registered ADS Router notification callback successfully.");
            }
        }
    }

    /// Unregester state notifications from the ADS Router to avoid bogging down the ADS router.
    fn unregister_ads_router_notification(&self) {
        unsafe {
            let err = AdsAmsUnRegisterRouterNotification();

            if err != 0 {
                log::error!(
                    "Failed to remove ADS Router notification callbacks: {}",
                    err
                );
            } else {
                log::info!("Removed ADS Router notification callback successfully.");
            }
        }
    }

    fn register_symbol_table_notification(&mut self) {
        let raw_address = &mut self.address as *mut AmsAddr;

        let changeFilter = AdsNotificationAttrib__bindgen_ty_1 {
            dwChangeFilter: 500000, // 500ms
        };

        let mut adsNotificationAttrib = AdsNotificationAttrib {
            cbLength: 1,
            nTransMode: nAdsTransMode_ADSTRANS_SERVERONCHA,
            nMaxDelay: 500000, // 500ms
            __bindgen_anon_1: changeFilter,
        };

        let raw_attr = &mut adsNotificationAttrib as *mut AdsNotificationAttrib;
        let ptr_notification_handle = &mut self.ads_symbol_table_notification_handle as *mut u32;

        unsafe {
            // NOTE: You can't use the Extended, thread-safe version of this function for registering
            // this particular notification.
            let state_noti_err = AdsSyncAddDeviceNotificationReq(
                raw_address,
                ADSIGRP_SYM_VERSION,
                0,
                raw_attr,
                Some(ads_symbol_table_callback),
                self.id,
                ptr_notification_handle,
            );

            if state_noti_err != 0 {
                log::error!(
                    "Failed to register ADS symbol table change notification: {}",
                    state_noti_err
                );
            } else {
                log::info!("Registered ADS symbol table notification successfully.");
            }
        }
    }

    /// Unregister the callback from the ADS router. Should be called during finalize
    /// to avoid bogging down the ADS Router.
    fn unregister_symbol_table_notification(&mut self) {
        if self.ads_symbol_table_notification_handle == 0 {
            return;
        }

        let raw_address = &mut self.address as *mut AmsAddr;

        unsafe {
            // NOTE: You can't use the Extended, thread-safe version of this function for registering
            // this particular notification.
            let state_noti_err = AdsSyncDelDeviceNotificationReq(
                raw_address,
                self.ads_symbol_table_notification_handle,
            );

            if state_noti_err != 0 {
                log::error!(
                    "Failed to remove ADS symbol_table notification callbacks: {}",
                    state_noti_err
                );
            } else {
                log::info!("Removed ADS State notification callback successfully.");
            }
        }
    }

    /// Upload and buffer all available symbols from the controller.
    #[allow(dead_code)]
    pub fn upload_all_symbols(&mut self) -> Result<(), anyhow::Error> {
        if let Some(res) = ads_symbol_loader::upload_symbols(&self.address, self.current_comms_port)
        {
            self.symbol_collection = res;
            Ok(())
        } else {
            Err(anyhow!(
                "Failed to upload symbols, which indicates a failure to connect to the device."
            ))
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
    pub fn find_symbols_in_domain(&self, domain: &str) -> Vec<AdsSymbolInfo> {
        self.symbol_collection.find_symbols_in_domain(domain)
    }

    /// Return the handle for a symbol in the PLC, it it exists.
    /// Returns handle > 0 if found, 0 if not.    
    #[allow(dead_code)]
    fn get_handle_by_name(&mut self, symbol_name: &str) -> Result<u32, anyhow::Error> {
        let raw_address = &mut self.address as *mut AmsAddr;
        let mut handle: u32 = 0;
        let ptr_handle = &mut handle as *mut u32 as *mut c_void;
        let ptr_name = symbol_name as *const str as *mut c_void;
        let mut bytes_read: u32 = 0;
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
                ptr_bytes_read,
            );

            if err != 0 {
                Err(anyhow!(
                    "Error reading handle for symbol {}: ADS error code {}",
                    symbol_name,
                    err
                ))
            } else {
                Ok(handle)
            }
        }
    }

    /// Upload information about a symbol from the PLC
    #[allow(dead_code)]
    pub fn upload_symbol_entry(
        &mut self,
        symbol_name: &str,
    ) -> Result<AdsSymbolEntry, anyhow::Error> {
        let raw_address = &mut self.address as *mut AmsAddr;
        let mut handle: u32 = 0;
        let ptr_handle = &mut handle as *mut u32 as *mut c_void;
        let ptr_name = symbol_name as *const str as *mut c_void;
        let mut bytes_read: u32 = 0;
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
                ptr_bytes_read,
            );

            if err != 0 {
                Err(anyhow!("Error reading symbol: ADS error code {}", err))
            } else {
                let info_err = AdsSyncReadWriteReqEx2(
                    self.current_comms_port,
                    raw_address,
                    ADSIGRP_SYM_INFOBYNAMEEX,
                    0,
                    std::mem::size_of::<AdsSymbolEntry>() as u32,
                    ptr_symbol_entry,
                    symbol_name.len() as u32,
                    ptr_name,
                    std::ptr::null_mut(),
                );

                if info_err == 0 {
                    Ok(symbol_entry)
                } else {
                    Err(anyhow!("Error reading symbol: ADS error code {}", err))
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
    pub fn get_symbol_entry(&mut self, symbol_name: &str) -> Result<AdsSymbolEntry, anyhow::Error> {
        if self.symbol_entries.contains_key(symbol_name) {
            Ok(self.symbol_entries[symbol_name])
        } else {
            self.upload_symbol_entry(symbol_name)
        }
    }

    /// Synchronous write of a value to the target PLC as a stream of bytes.
    /// This is the primaruy function for writing any value to the PLC.
    fn write_raw_bytes(&mut self, symbol_name: &str, bytes: &[u8]) -> Result<(), anyhow::Error> {
        let symbol_information = self.get_symbol_entry(symbol_name);
        match symbol_information {
            Ok(info) => {
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
                        ptr_buffer,
                    );

                    if rc != 0 {
                        Err(anyhow!(
                            "Failed to write symbol {} with error {}",
                            symbol_name,
                            rc
                        ))
                    } else {
                        Ok(())
                    }
                }
            }
            Err(err) => Err(anyhow!(
                "Failed to read information for symbol {} : {}",
                symbol_name,
                err
            )),
        }
    }

    /// Synchrnous read of a value in the target PLC, returned as a stream of bytes.
    /// This is the base function for reading any value from the PLC.
    fn read_raw_bytes(&mut self, symbol_name: &str) -> Result<Vec<u8>, anyhow::Error> {
        let raw_address = &mut self.address as *mut AmsAddr;
        // The "handle" or id within the controller that identifies a symbol.
        let mut handle: u32 = 0;
        // A void pointer to that handle we can pass to C functionc calls expecting a void*
        let ptr_handle = &mut handle as *mut u32 as *mut c_void;
        let ptr_name = symbol_name as *const str as *mut c_void;
        let mut bytes_read: u32 = 0;
        let ptr_bytes_read = &mut bytes_read as *mut u32;

        let symbol_information = self.get_symbol_entry(symbol_name);

        match symbol_information {
            Ok(info) => {
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
                        ptr_bytes_read,
                    );

                    if err != 0 {
                        // an error occurred.

                        Err(anyhow!("Error {} occurred!", err))
                    } else {
                        // Create a buffer of the appropriate sise, filled with 0's.
                        let mut buffer = vec![0u8; info.size as usize];
                        // Create a pointer to that under
                        let ptr_buffer = buffer.as_mut_ptr() as *mut c_void;

                        let read_err = AdsSyncReadReqEx2(
                            self.current_comms_port,
                            raw_address,
                            ADSIGRP_SYM_VALBYHND,
                            handle,
                            info.size,
                            ptr_buffer,
                            std::ptr::null_mut(),
                        );

                        if read_err != 0 {
                            // TODO: convert error code to error string.
                            Err(anyhow!(
                                "Error {} occurred reading value into buffer",
                                read_err
                            ))
                        } else {
                            Ok(buffer)
                        }
                    }
                }
            }
            Err(err) => Err(anyhow!(
                "Error : could not obtain handle to symbol {} : {}",
                symbol_name,
                err
            )),
        }
    }

    /// Synchronous read of a symbol based upon its name in the PLC.
    /// Complete, fully-qualified name required.
    pub fn read_symbol_by_name(
        &mut self,
        symbol_name: &str,
    ) -> Result<variant::VariantValue, anyhow::Error> {
        match self.read_raw_bytes(symbol_name) {
            Ok(buffer) => {
                //log::debug!("RES {} BUFFER: {:?}", buffer.len(), buffer);

                let symbol_information = self.get_symbol_entry(symbol_name);
                match symbol_information {
                    Ok(info) => match AdsDataTypeId::try_from(info.dataType) {
                        Ok(dt) => {
                            if let Ok(symbol_info) =
                                self.symbol_collection.get_symbol_info(symbol_name)
                            {
                                if let Some(ret) = ads_data::deserialize(
                                    &self.symbol_collection,
                                    &symbol_info,
                                    &buffer,
                                    &dt,
                                ) {
                                    Ok(ret)
                                } else {
                                    Err(anyhow!("Failed to read symbol: {}", symbol_name))
                                }
                            } else {
                                Err(anyhow!(
                                    "Failed to find symbol {} in uploaded symbol table.",
                                    symbol_name
                                ))
                            }
                        }
                        Err(err) => Err(anyhow!(
                            "Unsupported type {} for symbol: {}",
                            err,
                            symbol_name
                        )),
                    },
                    Err(err) => Err(anyhow!("{}", err)),
                }
            }
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
    pub fn read_symbol_value<T: AsBytes + FromBytes + Default>(
        &mut self,
        symbol_name: &str,
    ) -> Result<T, anyhow::Error> {
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
                    None => Err(anyhow!(
                        "Failed to convert buffer to type. Buffer size: {}",
                        buffer.len()
                    )),
                }
            }
            Err(err) => Err(anyhow!("{}", err)),
        }
    }

    /// Read a symbol value specifically as a string.
    pub fn read_symbol_string_value(&mut self, symbol_name: &str) -> Result<String, anyhow::Error> {
        match self.read_raw_bytes(symbol_name) {
            Ok(buffer) => vec_to_string(&buffer),
            Err(err) => Err(anyhow!("{}", err)),
        }
    }

    /// Write the value of the specified type to the target. The type is serialized and transmitted.
    /// Dynamic types (like Vec or String) are not supported.
    pub fn write_symbol_value<T: AsBytes>(
        &mut self,
        symbol_name: &str,
        value: T,
    ) -> Result<(), anyhow::Error> {
        let bytes = value.as_bytes();

        // Here, you would write the logic to send `bytes` to the PLC.
        // This could involve sending the bytes over a network, writing to a file, etc.
        // For this example, let's assume a function `write_raw_bytes` does this job.
        self.write_raw_bytes(symbol_name, bytes)
    }

    /// Write a string slice to the target. The string is converted to bytes and transmitted.
    /// Note that the typical string in the PLC will be using 8-bit ASCII text.
    pub fn write_symbol_string_value(
        &mut self,
        symbol_name: &str,
        value: &str,
    ) -> Result<(), anyhow::Error> {
        let bytes = value.as_bytes();

        // Here, you would write the logic to send `bytes` to the PLC.
        // This could involve sending the bytes over a network, writing to a file, etc.
        // For this example, let's assume a function `write_raw_bytes` does this job.
        self.write_raw_bytes(symbol_name, bytes)
    }

    /// Write a symbol with the value contained in the Variant. The underlying value of VariantValue is
    /// expected to match the target symbol in the PLC exactly; this function will not convert.
    pub fn write_symbol_variant_value(
        &mut self,
        symbol_name: &str,
        value: &VariantValue,
    ) -> Result<(), anyhow::Error> {
        if let Ok(symbol_info) = self.symbol_collection.get_symbol_info(symbol_name) {
            match serialize(&self.symbol_collection, &symbol_info, value) {
                Ok(bytes) => {
                    //log::debug!("\n{:?}\n", bytes);
                    self.write_raw_bytes(symbol_name, &bytes)
                }
                Err(err) => Err(anyhow!("Failed to serialize value: {}", err)),
            }
        } else {
            Err(anyhow!(
                "Failed to find information on symbol {}",
                symbol_name
            ))
        }
    }

    /// Converts a JSON value to a corresponding `VariantValue` based on ADS symbol information.
    ///
    /// This function interprets a `serde_json::Value` according to the type and structure
    /// described by `AdsSymbolInfo`. It supports basic data types, arrays, and nested structures
    /// by utilizing detailed type information from a symbol collection. For arrays, it expects
    /// the JSON value to be an array and converts each element recursively. For structured types
    /// (`BigType`), it expects a JSON object and processes each field according to the structure's
    /// definition. Errors are returned for type mismatches or unsupported data types.
    ///
    /// # Parameters
    /// - `json_val`: The JSON value to convert.
    /// - `symbol_info`: Metadata describing the symbol's data type and structure.
    ///
    /// # Returns
    /// - `Ok(VariantValue)`: The converted value wrapped in a `VariantValue` enum.
    /// - `Err(anyhow::Error)`: Error if the conversion fails due to type mismatches or missing information.
    pub fn convert_json_to_variant(
        &self,
        json_val: &serde_json::Value,
        symbol_info: &AdsSymbolInfo,
    ) -> Result<VariantValue, anyhow::Error> {
        if let Some(dt) = self
            .symbol_collection
            .get_fundamental_type_info(symbol_info)
        {
            if symbol_info.is_array {
                match json_val {
                    serde_json::Value::Array(array) => {
                        let mut variant_array = Vec::new();
                        for item in array {
                            let variant = self.convert_json_value_to_variant(item, &dt)?;
                            variant_array.push(variant);
                        }
                        Ok(VariantValue::Array(variant_array))
                    }
                    _ => Err(anyhow::anyhow!(
                        "Expected a JSON array for array type symbol"
                    )),
                }
            } else if let Ok(type_id) = AdsDataTypeId::try_from(dt.data_type) {
                //log::debug!("type_id: {:?}", type_id);

                match type_id {
                    AdsDataTypeId::Void => {
                        Err(anyhow::anyhow!("Cannot write to a void type value"))
                    }
                    AdsDataTypeId::Int8 => json_val
                        .as_i64()
                        .map(|x| VariantValue::SByte(x as i8))
                        .ok_or_else(|| anyhow::anyhow!("Invalid value for Int8")),
                    AdsDataTypeId::UInt8 => json_val
                        .as_u64()
                        .map(|x| VariantValue::Byte(x as u8))
                        .ok_or_else(|| anyhow::anyhow!("Invalid value for UInt8")),
                    AdsDataTypeId::Int16 => json_val
                        .as_i64()
                        .map(|x| VariantValue::Int16(x as i16))
                        .ok_or_else(|| anyhow::anyhow!("Invalid value for Int16")),
                    AdsDataTypeId::UInt16 => json_val
                        .as_u64()
                        .map(|x| VariantValue::UInt16(x as u16))
                        .ok_or_else(|| anyhow::anyhow!("Invalid value for UInt16")),
                    AdsDataTypeId::Int32 => json_val
                        .as_i64()
                        .map(|x| VariantValue::Int32(x as i32))
                        .ok_or_else(|| anyhow::anyhow!("Invalid value for Int32")),
                    AdsDataTypeId::UInt32 => json_val
                        .as_u64()
                        .map(|x| VariantValue::UInt32(x as u32))
                        .ok_or_else(|| anyhow::anyhow!("Invalid value for UInt32")),
                    AdsDataTypeId::Int64 => json_val
                        .as_i64()
                        .map(VariantValue::Int64)
                        .ok_or_else(|| anyhow::anyhow!("Invalid value for Int64")),
                    AdsDataTypeId::UInt64 => json_val
                        .as_u64()
                        .map(VariantValue::UInt64)
                        .ok_or_else(|| anyhow::anyhow!("Invalid value for UInt64")),
                    AdsDataTypeId::Real32 => {
                        if let Some(val) = json_val.as_f64() {
                            return Ok(VariantValue::Real32(val as f32));
                        } else {
                            return Err(anyhow::anyhow!("Invalid value for Real32"));
                        }
                    }
                    AdsDataTypeId::Real64 => {
                        if let Some(val) = json_val.as_f64() {
                            return Ok(VariantValue::Real64(val));
                        } else {
                            return Err(anyhow::anyhow!("Invalid value for Real32"));
                        }
                    }
                    AdsDataTypeId::String | AdsDataTypeId::WString => json_val
                        .as_str()
                        .map(|s| VariantValue::String(s.to_string()))
                        .ok_or_else(|| anyhow::anyhow!("Invalid value for String types")),
                    AdsDataTypeId::Bit => json_val
                        .as_bool()
                        .map(VariantValue::Bit)
                        .ok_or_else(|| anyhow::anyhow!("Invalid value for bool")),
                    AdsDataTypeId::BigType => {
                        if let serde_json::Value::Object(map) = json_val {
                            let mut object = IndexMap::new();
                            for field in &dt.fields {
                                let field_value = map.get(&field.name).ok_or_else(|| {
                                    anyhow::anyhow!("Field '{}' missing in input JSON", field.name)
                                })?;
                                let field_variant =
                                    self.convert_json_to_variant(field_value, field)?;
                                object.insert(field.name.clone(), field_variant);
                            }
                            Ok(VariantValue::Object(Box::new(object)))
                        } else {
                            Err(anyhow::anyhow!(
                                "Expected a JSON object for structure serialization"
                            ))
                        }
                    }
                    _ => Err(anyhow::anyhow!("Unsupported data type ID")),
                }
            } else {
                return Err(anyhow!("Unsupported data type id {}", dt.data_type));
            }
        } else {
            Err(anyhow::anyhow!(
                "Failed to retrieve fundamental type info for {}",
                symbol_info.name
            ))
        }
    }

    /// Converts an individual JSON value to a `VariantValue` based on specific ADS data type information.
    ///
    /// This helper function is a utility for converting a single JSON value to a `VariantValue`
    /// as dictated by the ADS data type (`AdsDataTypeInfo`). It handles type conversions for basic data types,
    /// including integers, floating points, strings, and booleans, and also manages structured types (`BigType`)
    /// by recursive conversion of each field in a JSON object. The function is used primarily within
    /// array processing or nested structure conversions in the main `convert_json_to_variant` function.
    ///
    /// # Parameters
    /// - `json_val`: The JSON value to convert.
    /// - `dt`: The detailed data type information used for the conversion.
    ///
    /// # Returns
    /// - `Ok(VariantValue)`: The converted value wrapped in a `VariantValue` enum.
    /// - `Err(anyhow::Error)`: Error if the conversion fails due to type mismatches or unsupported data types.
    fn convert_json_value_to_variant(
        &self,
        json_val: &serde_json::Value,
        dt: &AdsDataTypeInfo,
    ) -> Result<VariantValue, anyhow::Error> {
        if let Ok(type_id) = AdsDataTypeId::try_from(dt.data_type) {
            match type_id {
                AdsDataTypeId::Void => Err(anyhow::anyhow!("Cannot write to a void type value")),
                AdsDataTypeId::Int8 => json_val
                    .as_i64()
                    .map(|x| VariantValue::SByte(x as i8))
                    .ok_or_else(|| anyhow::anyhow!("Invalid value for Int8")),
                AdsDataTypeId::UInt8 => json_val
                    .as_u64()
                    .map(|x| VariantValue::Byte(x as u8))
                    .ok_or_else(|| anyhow::anyhow!("Invalid value for UInt8")),
                AdsDataTypeId::Int16 => json_val
                    .as_i64()
                    .map(|x| VariantValue::Int16(x as i16))
                    .ok_or_else(|| anyhow::anyhow!("Invalid value for Int16")),
                AdsDataTypeId::UInt16 => json_val
                    .as_u64()
                    .map(|x| VariantValue::UInt16(x as u16))
                    .ok_or_else(|| anyhow::anyhow!("Invalid value for UInt16")),
                AdsDataTypeId::Int32 => json_val
                    .as_i64()
                    .map(|x| VariantValue::Int32(x as i32))
                    .ok_or_else(|| anyhow::anyhow!("Invalid value for Int32")),
                AdsDataTypeId::UInt32 => json_val
                    .as_u64()
                    .map(|x| VariantValue::UInt32(x as u32))
                    .ok_or_else(|| anyhow::anyhow!("Invalid value for UInt32")),
                AdsDataTypeId::Int64 => json_val
                    .as_i64()
                    .map(VariantValue::Int64)
                    .ok_or_else(|| anyhow::anyhow!("Invalid value for Int64")),
                AdsDataTypeId::UInt64 => json_val
                    .as_u64()
                    .map(VariantValue::UInt64)
                    .ok_or_else(|| anyhow::anyhow!("Invalid value for UInt64")),
                AdsDataTypeId::Real32 => json_val
                    .as_f64()
                    .map(|x| VariantValue::Real32(x as f32))
                    .ok_or_else(|| anyhow::anyhow!("Invalid value for Real32")),
                AdsDataTypeId::Real64 => json_val
                    .as_f64()
                    .map(VariantValue::Real64)
                    .ok_or_else(|| anyhow::anyhow!("Invalid value for Real64")),
                AdsDataTypeId::String | AdsDataTypeId::WString => json_val
                    .as_str()
                    .map(|s| VariantValue::String(s.to_string()))
                    .ok_or_else(|| anyhow::anyhow!("Invalid value for String types")),
                AdsDataTypeId::Bit => json_val
                    .as_bool()
                    .map(VariantValue::Bit)
                    .ok_or_else(|| anyhow::anyhow!("Invalid value for bool")),
                AdsDataTypeId::BigType => {
                    if let serde_json::Value::Object(map) = json_val {
                        let mut object = IndexMap::new();
                        for field in &dt.fields {
                            let field_value = map.get(&field.name).ok_or_else(|| {
                                anyhow::anyhow!("Field '{}' missing in input JSON", field.name)
                            })?;
                            let field_variant = self.convert_json_to_variant(field_value, field)?;
                            object.insert(field.name.clone(), field_variant);
                        }
                        Ok(VariantValue::Object(Box::new(object)))
                    } else {
                        Err(anyhow::anyhow!(
                            "Expected a JSON object for structure serialization"
                        ))
                    }
                }
                _ => Err(anyhow::anyhow!("Unsupported data type ID")),
            }
        } else {
            Err(anyhow!("Unsupported data type id {}", dt.data_type))
        }
    }

    /// Writes a JSON value to a symbol in the remote ADS device.
    ///
    /// This function takes a symbol name and a JSON value, converts the JSON to a `VariantValue`
    /// using `convert_json_to_variant`, and then writes it to the corresponding symbol in the remote ADS device.
    /// The function is intended for updating values in an ADS device where the symbol information
    /// and corresponding JSON values are known. Errors are returned if the symbol cannot be located
    /// or if the conversion/writing process fails.
    ///
    /// # Parameters
    /// - `symbol_name`: The name of the symbol to write to.
    /// - `value`: The JSON value to write.
    ///
    /// # Returns
    /// - `Ok(())`: On successful write.
    /// - `Err(anyhow::Error)`: If there is an error in locating the symbol, converting the value, or during write.    
    pub fn write_symbol_json_value(
        &mut self,
        symbol_name: &str,
        value: &serde_json::Value,
    ) -> Result<(), anyhow::Error> {
        match self.symbol_collection.get_symbol_info(symbol_name) {
            Ok(info) => match self.convert_json_to_variant(value, &info) {
                Ok(var) => self.write_symbol_variant_value(symbol_name, &var),
                Err(err) => Err(anyhow!("Error writing symbol {} : {}", symbol_name, err)),
            },
            Err(err) => Err(anyhow!(
                "Failed to locate info on symbol {} : {}",
                symbol_name,
                err
            )),
        }
    }

    /// Register a symbol for on-data-change notification.
    /// Returns true if successful, false if not.
    pub fn register_symbol(
        &mut self,
        symbol_name: &str,
        options: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), anyhow::Error> {
        if let Ok(item) = get_registered_symbol_by_name(symbol_name) {
            if item.notification_handle != 0 {
                // already registered. Do nothing.
                log::info!("Symbol {} is already registerd.", symbol_name);
                return Ok(());
            }
        }

        if let Ok(symbol) = self.symbol_collection.get_symbol_info(symbol_name) {
            // Cycle time determines the maximum update frequency for a registered symbol.
            // For symbols that change constantly, like motion position or load cell readings,
            // cycle time should be increased to avoid crushing the user interface.
            let mut cycle_time_ms: u32 = 50;

            if options.contains_key("cycle_time") {
                if let Some(val) = options["cycle_time"].as_u64() {
                    cycle_time_ms = val as u32;
                }
            }

            if cycle_time_ms < 10 {
                cycle_time_ms = 10;
            }

            // unit = 100ns, 10msec = 100000
            let cycle_time_ns = cycle_time_ms * 10000;

            let symbol_data_type;
            if let Ok(id) = AdsDataTypeId::try_from(symbol.type_id) {
                symbol_data_type = id;
            } else {
                return Err(anyhow!(
                    "Failed to extract data type for symbol {}",
                    symbol_name
                ));
            }

            log::info!(
                "Attempting to register {} with type {:?}",
                symbol_name,
                symbol_data_type
            );

            //
            // Bindgen somehow flubbed the generation of the nCycleTime member, and turned it into
            // a union.
            //
            let nCycleTime = AdsNotificationAttrib__bindgen_ty_1 {
                nCycleTime: cycle_time_ns,
            }; // unit = 100ns, 10msec = 100000

            let mut attrib = AdsNotificationAttrib {
                cbLength: symbol.size,
                nTransMode: nAdsTransMode_ADSTRANS_SERVERONCHA,
                nMaxDelay: 0,
                __bindgen_anon_1: nCycleTime, // nCycleTimeMember
            };

            match self.get_handle_by_name(symbol_name) {
                Ok(handle) => {
                    let raw_address = &mut self.address as *mut AmsAddr;
                    let mut notification_handle: u32 = 0;
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

                        info!(
                            "Registering notification for {} with handle {}",
                            symbol_name, handle
                        );

                        let notitication_err = AdsSyncAddDeviceNotificationReqEx(
                            self.current_comms_port,
                            raw_address,
                            ADSIGRP_SYM_VALBYHND,
                            handle,
                            ptr_attrib,
                            Some(ads_value_callback), // see note above
                            handle,
                            ptr_notification_handle,
                        );

                        if notitication_err != 0 {
                            return Err(anyhow!("Failed to register handle: {}", notitication_err));
                        }
                    }

                    add_registered_symbol(RegisteredSymbol {
                        handle,
                        notification_handle,
                        name: symbol_name.to_string(),
                        data_type_id: symbol_data_type,
                        options: options.clone(),
                    });

                    Ok(())
                }
                Err(err) => Err(err),
            }
        } else {
            Err(anyhow!(
                "The requested symbol {} does not exist in the controller. 
                Please check symbol name and connection properties.",
                symbol_name
            ))
        }
    }

    /// Unregister a symbol that was registered for notification. It's important to unregister
    /// a notification before closing connections, or the ADS router will get bogged down.
    pub fn unregister_symbol(&mut self, symbol_name: &str) -> Result<(), anyhow::Error> {
        let raw_address = &mut self.address as *mut AmsAddr;
        match get_registered_symbol_by_name(symbol_name) {
            Ok(sym) => unsafe {
                let notitication_err = AdsSyncDelDeviceNotificationReqEx(
                    self.current_comms_port,
                    raw_address,
                    sym.notification_handle,
                );

                if notitication_err != 0 {
                    Err(anyhow!(
                        "Failed to remove notification: {}",
                        notitication_err
                    ))
                } else {
                    info!("Removed notification for {}", symbol_name);

                    if let Err(err) = remove_registered_symbol_by_name(symbol_name) {
                        error!(
                            "Failed to remove notification symbol from local registry: {}",
                            err
                        );
                    }

                    Ok(())
                }
            },
            Err(err) => {
                if let Err(err) = remove_registered_symbol_by_name(symbol_name) {
                    error!(
                        "Failed to remove notification symbol from local registry: {}",
                        err
                    );
                }

                Err(err)
            }
        }
    }

    /// Unregister notifications for all symbols registered with the ADS Router.
    /// This function should be called before closing the connection to the ADS Router.
    pub fn unregister_all_symbols(&mut self) {
        while let Some(item) = get_last_registered_symbol() {
            if let Err(err) = self.unregister_symbol(&item.name) {
                log::error!("Failed to unregister symbol {} : {}", item.name, err);

                // We still need to remove it from our interal list, or we'll loop here endlessly.
                let _ = remove_registered_symbol_by_name(&item.name);
            } else {
                log::info!("Unregistered symbol {}", item.name);
            }
        }
    }

    /// Re-register notifications for all symbols registered with the ADS Router.
    /// This becomes necessary if the symbol table changes.
    pub fn reregister_all_symbols(&mut self) {
        if self.symbol_reregistration_count > 0 {
            let mut items: Vec<(String, serde_json::Map<String, serde_json::Value>)> = Vec::new();

            {
                // Build a list of symbol names. We can't hold the mutex while registering and unregistering
                // symbols, or we'll deadlock.
                let guard = SYMBOL_MANAGER.registered_symbols.lock().unwrap();

                for item in guard.iter() {
                    items.push((item.name.clone(), item.options.clone()));
                }
            }

            // Upload the latest information on the symbol table.
            match self.upload_all_symbols() {
                Ok(_) => {
                    // Now loop through the symbol names and re-register.
                    for item in items {
                        if self.unregister_symbol(&item.0).is_err() {
                            let _ = remove_registered_symbol_by_name(&item.0);
                        }

                        if let Err(err) = self.register_symbol(&item.0, &item.1) {
                            log::error!("Failed to re-register symbol {} : {}", item.0, err);
                        } else {
                            log::info!("Re-registered symbol {}", item.0);
                        }
                    }
                }
                Err(err) => {
                    log::error!("Re-register failed: could not upload symbol table: {}", err);
                    log::error!("Reconnection is required.");
                }
            }
        }

        self.symbol_reregistration_count += 1;
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
    hUser: ::std::os::raw::c_ulong,
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
    hUser: ::std::os::raw::c_ulong,
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
///   can, and often will, send out the initial value for the symbol
///   before sending back the notification handle to the method that registered
///   the on-data-change callback.
///
/// - Callback signatures typically get generated as Option<unsafe extern "C" fn ...>
///   in bindgen. So when you write your callbacks in Rust, you obviously need to decorate
///   them with unsafe extern "C" (they don’t need to be pub), and then when you pass them
///   to the C library you just wrap the name in Some.
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
    hUser: ::std::os::raw::c_ulong,
) {
    let id = hUser;

    if get_registered_symbol_by_handle(id).is_ok() {
        let data_size = (*pNotification).cbSampleSize as usize;
        let data_slice = slice::from_raw_parts((*pNotification).data.as_ptr(), data_size);

        let mut global_sender = SYMBOL_MANAGER.sender.lock().unwrap();

        // Collect indices of the failed sends to remove them later
        //let mut failed_indices = Vec::new();

        for (index, sender) in global_sender.iter().enumerate() {
            let dcei = RouterNotificationEventInfo {
                event_type: EventInfoType::DataChange,
                id,
                data: Vec::from(data_slice),
            };

            if sender.try_send(dcei).is_err() {
                log::error!("Failed to send on notification channel for DATA CHANGE!");
                //log::error!("Failed to send on notification channel for DATA CHANGE, channel will be removed.");
                //failed_indices.push(index);
            }
        }

        // Remove channels in reverse order to maintain correct indices
        // for index in failed_indices.into_iter().rev() {
        //     global_sender.remove(index);
        // }
    } else {
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
    hUser: ::std::os::raw::c_ulong,
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
    hUser: ::std::os::raw::c_ulong,
) {
    handle_ads_state_callback(pAddr, pNotification, hUser);
}

/// Callback when the state of the ADS Router has changed. This is a global event, as
/// the Router is the hub connecting us to all targets, not just the particular client instance.
unsafe fn handle_ads_state_callback(
    _pAddr: *mut AmsAddr,
    pNotification: *mut AdsNotificationHeader,
    hUser: ::std::os::raw::c_ulong,
) {
    let id = hUser;

    let global_sender = SYMBOL_MANAGER.sender.lock().unwrap();

    // Assuming `cbSampleSize` gives the size of the data in bytes
    let data_size = (*pNotification).cbSampleSize as usize;

    // Create a slice from the `data` pointer for safe access
    // This is safe because we trust `cbSampleSize` to give us the correct length of the data
    let data_slice = slice::from_raw_parts((*pNotification).data.as_ptr(), data_size);

    if let Some(state) = i16::read_from(data_slice) {
        for i in 0..global_sender.len() {
            let dcei = RouterNotificationEventInfo {
                event_type: EventInfoType::AdsState,
                id,
                data: state.to_le_bytes().to_vec(),
            };

            match global_sender[i].try_send(dcei) {
                Ok(_) => {}
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
unsafe extern "stdcall" fn ads_router_callback(state: ::std::os::raw::c_long) {
    handle_ads_router_callback(pAddr, pNotification, hUser);
}

/// Callback from the 32-bit Beckhoff Ads DLL signalling a change in the target state.
/// Usually, this means the PLC changed its run state.
///
/// # Arguments
/// * `state` - Updated state of the ADS router

///
#[cfg(target_arch = "x86_64")]
unsafe extern "C" fn ads_router_callback(state: ::std::os::raw::c_long) {
    handle_ads_router_callback(state);
}

/// Handles the callback from the ADS router that it has changed state.
/// Note that the callback will only be issued when the state changes, and there
/// will be no initial callback when the connection is made, unlike other notifications.
unsafe fn handle_ads_router_callback(state: ::std::os::raw::c_long) {
    let global_sender = SYMBOL_MANAGER.sender.lock().unwrap();

    for i in 0..global_sender.len() {
        let dcei = RouterNotificationEventInfo {
            event_type: EventInfoType::RouterState,
            id: 32767, // BROADCAST
            data: state.to_le_bytes().to_vec(),
        };

        match global_sender[i].try_send(dcei) {
            Ok(_) => {}
            Err(err) => log::error!("Failed to send on CHANNEL for ADS ROUTER CHANGE: {}", err),
        }
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
unsafe extern "stdcall" fn ads_symbol_table_callback(
    pAddr: *mut AmsAddr,
    pNotification: *mut AdsNotificationHeader,
    hUser: ::std::os::raw::c_ulong,
) {
    ads_symbol_table_callback(pAddr, pNotification, hUser);
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
unsafe extern "C" fn ads_symbol_table_callback(
    pAddr: *mut AmsAddr,
    pNotification: *mut AdsNotificationHeader,
    hUser: ::std::os::raw::c_ulong,
) {
    handle_ads_symbol_table_callback(pAddr, pNotification, hUser);
}

/// Callback when the state of the ADS Router has changed. This is a global event, as
/// the Router is the hub connecting us to all targets, not just the particular client instance.
unsafe fn handle_ads_symbol_table_callback(
    _pAddr: *mut AmsAddr,
    _pNotification: *mut AdsNotificationHeader,
    _hUser: ::std::os::raw::c_ulong,
) {
    let global_sender = SYMBOL_MANAGER.sender.lock().unwrap();

    for i in 0..global_sender.len() {
        let dcei = RouterNotificationEventInfo {
            event_type: EventInfoType::SymbolTableChange,
            id: 0,
            data: Vec::new(),
        };

        match global_sender[i].try_send(dcei) {
            Ok(_) => {}
            Err(err) => {
                log::error!(
                    "Failed to send on CHANNEL for ADS SYMBOL TABLE CHANGE: {}",
                    err
                );
            }
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
        let val: i32 = AdsGetDllVersion();
        info!("The ADS Dll version is {}", val);
        val
    }
}

/// Returns the AMS address of the local TwinCAT system.
pub fn get_local_address() -> AmsAddr {
    let mut local_addr = AmsAddr::default();

    unsafe {
        AdsGetLocalAddress(&mut local_addr);
    }

    local_addr
}

/// Convert a string to an AmsAddr structure to be used with the
/// TwinCAT router. Returns None if the address is at all invalid.
pub fn to_ams_addr(s: &str) -> Option<AmsAddr> {
    //
    // First, split the address into tokens separated by the . character
    let tokens = s.trim().split_terminator('.').collect::<Vec<_>>();

    if tokens.len() != 6 {
        log::warn!(
            "{} does not contain 6 tokens. Num tokens: {}",
            s,
            tokens.len()
        );
        return None;
    }

    let mut ret = AmsAddr::default();

    for (i, token) in tokens.iter().enumerate().take(6) {
        match u8::from_str(tokens[i]) {
            Ok(b) => ret.netId.b[i] = b,
            Err(err) => {
                log::warn!("Error parsing {}: {}", tokens[i], err);
                return None;
            }
        }
    }

    Some(ret)
}

#[test]
fn hello_ads() {
    println!("Hello, ADS!");

    unsafe {
        let val = AdsGetDllVersion();

        println!("The ADS Dll version is {}", val);

        let client_port = crate::AdsPortOpen();

        println!("The ADS client port is {}", client_port);

        let mut local_addr = AmsAddr::default();

        AdsGetLocalAddress(&mut local_addr);

        println!(
            "local addr is {}.{}.{}.{}.{}.{}",
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
