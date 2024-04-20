//
// Copyright (C) 2024 Automated Design Corp. All Rights Reserved.
// Created Date: 2024-04-06 10:24:11
// -----
// Last Modified: 2024-04-20 08:53:19
// -----
// 
//

//! Quick communications test to evalutate operation of the library.
//! We include a test project in the source directory that can be
//! downloaded to a TC PLC instance for use in testing this
//! 

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use log::{info, error};
use mechutil::variant::VariantValue;
use simplelog::*;

use twincatads_rs::client::{AdsClient, MaxString, AdsState, RouterState};


/// Main entry point of the program.
#[tokio::main]
async fn main() {

    // Configure logging
    CombinedLogger::init(vec![
        TermLogger::new(
            LevelFilter::Debug,
            Config::default(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        )
    ])
    .unwrap();

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();    


    
    let (mut client, mut rx) = AdsClient::new();
    // Supply AMS Address. If not set, localhost is used.
    //client.set_address("192.168.127.1.1.1");
    client.set_address("5.78.94.236.1.1");
    
    // Supply ADS port. If not set, the default of 851 is used.
    // You should generally use the default.
    client.set_port(851);
     
    // Make the connection to the ADS router
    client.initialize();
    
    // Write a value to a symbol in the PLC
    if let Err(err) = client.write_symbol_string_value(
        "GM.sTarget",
        "There is no spoon."
    ) {
        println!("An error occurred writing the tag: {}", err);
    }


    if let Err(err) = client.write_symbol_value::<MaxString>(
        "GM.sTarget",
        MaxString::from_string("I'm not even supposed to be here today.")
    ) {
        println!("An error occurred writing <MaxString> for: {}", err);
    }


    match client.read_symbol_value::<MaxString>("GM.sEcho") {
        Ok(val) => info!("STRING VALUE: {:?}", val.to_string()),
        Err(err) => error!("I failed to read the string tag: {}", err)
    }

    log::info!("client.read_symbol_value::<MaxString>(\"GK.sReadTest\") will fail because size of GK.sReadTest is smaller than MaxString...");
    match client.read_symbol_value::<MaxString>("GK.sReadTest") {
        Ok(val) => info!("STRING VALUE: {:?}", val.to_string()),
        Err(err) => error!("I failed to read the string tag: {}", err)
    }    

    log::info!("However, read_symbol_string_value(\"GK.sReadTest\") works fine.");    
    match client.read_symbol_string_value("GK.sReadTest") {
        Ok(val) => info!("STRING VALUE: {:?}", val.to_string()),
        Err(err) => error!("I failed to read the string tag: {}", err)
    }        
    
    match client.read_symbol_string_value("GM.sEcho") {
        Ok(val) => info!("STRING VALUE: {:?}", val.to_string()),
        Err(err) => error!("I failed to read the string tag: {}", err)
    }    
    

    const NOTIFY_TAG :&str = "GM.aTestArray";
    const WRITE_TAG : &str = "GM.aArrayWriteTarget";

    
    if let Err(err) = client.register_symbol(NOTIFY_TAG) {
        error!("Failed to register symbol: {}", err);
    }
    else {

        // Second channel for sending notifications back to the main thread
        let (tx_main, mut rx_main) = mpsc::channel(100);

        let t1_running = running.clone();

        // On the parent context (e.g., main thread), listen for notifications
        let t1 = tokio::spawn(async move {
            
            while t1_running.load(Ordering::Relaxed) {
                let timeout_duration = Duration::from_millis(4000);
                let result = {
                    
                    tokio::time::timeout(timeout_duration, rx.recv()).await
                };
    
                match result {
                    Ok(Some(notification)) => {
                        if let Err(err) = tx_main.send(notification).await {
                            log::error!("Failed to send notification to main thread: {}", err );
                            break;
                        }
                    },
                    Ok(None) => {
                        log::info!("T1 Channel closed.");
                        break;
                    },
                    Err(_) => {
                        log::info!("T1 Operation timed out");
                    }
                }
            }

            log::info!("T1 task has closed.");
        });


        let mut blink = false;

        let t2 = tokio::spawn(async move {
            while running.load(Ordering::SeqCst) {
                match rx_main.recv().await {   // recv_timeout(timeout) {
                    Some(notification) => {

                        log::info!("Notification type {:?} received in main thread",  notification.event_type);
                        
                        match notification.event_type {
                            twincatads_rs::client::client_types::EventInfoType::Invalid => {
                                log::error!("Invalid notification received.");
                            },
                            twincatads_rs::client::client_types::EventInfoType::DataChange => {

                                if let Err(err) = client.write_symbol_variant_value(WRITE_TAG, &notification.value) {
                                    log::error!("Failed to write struct to client: {}", err);
            
                                }
                                else {
                                    blink = !blink;
                                    let var_blink = VariantValue::Bit(!blink);
            
                                    if let Err(err) = client.write_symbol_variant_value("GM.bBoolTarget", &var_blink) {
                                        log::error!("Failed to write bool from variant to client: {}", err);
                                    }
            
                                }
            
                            },
                            twincatads_rs::client::client_types::EventInfoType::AdsState => {
                                match AdsState::from(notification.value) {
                                    AdsState::Running=> log::info!("Target device is RUNNING"),
                                    AdsState::Stopped => log::info!("Target device is STOPPED"),                                
                                    AdsState::Unknown => log::info!("Target device is in an unknown state."),
                                }
                            },
                            twincatads_rs::client::client_types::EventInfoType::RouterState => {
                                match RouterState::from(notification.value) {
                                    RouterState::Started => log::info!("Router is RUNNING"),
                                    RouterState::Stopped => log::info!("Router is STOPPED"),
                                    RouterState::Removed => log::info!("Router has been removed!"),
                                    RouterState::Unknown => log::info!("Router is in an unknown state."),
                                }
                            },
                        }


                    },
                    None => {
                        // Channel is disconnected, probably should exit
                        log::info!("Channel disconnected, exiting loop.");
                        break;
                    }
                    // Err(e) => {
                    //     if e != mpsc::RecvTimeoutError::Timeout {
                    //         // Channel is disconnected, probably should exit
                    //         println!("Channel disconnected, exiting loop.");
                    //         break;
                    //     }
                    // },
                }
            }        

            log::info!("Shutting down ADS client...");
            // Make sure the ADS client is closed.
            client.finalize().await;

            log::info!("T2 task has closed.");
            
        });

        
        // if let Err(err) = client.unregister_symbol(NOTIFY_TAG) {
        //     error!("Failed to unregister symbol: {} ", err);
        // }

        // Handling of ctrl+C or SIGINT
        let ctrl_c_future = tokio::signal::ctrl_c();
        log::info!("ADS Client up and running. Press Ctrl+C to exit.");

        // Await the Ctrl+C signal
        let _ = ctrl_c_future.await;
        log::info!("*** Ctrl+C received! Shutdown initiating... ***");

        r.store(false, Ordering::SeqCst);

        if let Err(err) = t1.await {
            log::error!("Failed to wait for T1 to shut down. May not have shut down properly. {} ", 
                err
            );
        }
        else {
            log::info!("T1 task reports closed. Checking T2...");
        }


        if let Err(err) = t2.await {
            log::error!("Failed to wait for T2 to shut down. May not have shut down properly. {} ", 
                err
            );
        }
        else {
            log::info!("T2 task reports closed. Shutdown complete.");
        }

    }
    
    info!("Goodbye!");
    
}