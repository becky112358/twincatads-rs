//
// Copyright (C) 2024 Automated Design Corp.. All Rights Reserved.
// Created Date: 2024-04-06 10:24:11
// -----
// Last Modified: 2024-04-11 14:39:45
// -----
// 
//

//! Quick communications test to evalutate operation of the library.
//! We include a test project in the source directory that can be
//! downloaded to a TC PLC instance for use in testing this
//! 

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::{thread, time};
use anyhow::{anyhow, Error};
use log::{info, error, debug};
use mechutil::variant::VariantValue;
use simplelog::*;

use twincatads_rs::client::adsclient::AdsClient;
use twincatads_rs::client::client_types::MaxString;


/// Main entry point of the program.
fn main() {

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

    // Setup Ctrl+C handler
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    }).expect("Error setting Ctrl-C handler");

    
    let (mut client, rx) = AdsClient::new();
    // Supply AMS Address. If not set, localhost is used.
    client.set_address("192.168.127.1.1.1");
    // client.set_address("5.78.94.236.1.1");
    
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
        let (tx_main, rx_main) = mpsc::channel();

        // On the parent context (e.g., main thread), listen for notifications
        thread::spawn(move || {

            //
            // When you call .iter() on a Receiver, you get an iterator that blocks waiting for new messages, 
            // and it continues until the channel is closed.
            //
            for notification in rx.iter() {
                //println!("Notification received: {} : {}", notification.name, notification.value);

                tx_main.send(notification.value).expect("Failed to send the notification back to the main thread");

                // Handle the notification...
            }
        });


        let timeout = time::Duration::from_millis(100); 

        let mut blink = false;
        while running.load(Ordering::SeqCst) {
            match rx_main.recv_timeout(timeout) {
                Ok(notification_value) => {
                    println!("Notification value received in main thread: {}", notification_value);


                    if let Err(err) = client.write_symbol_variant_value(WRITE_TAG, &notification_value) {
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
                Err(e) => {
                    if e != mpsc::RecvTimeoutError::Timeout {
                        // Channel is disconnected, probably should exit
                        println!("Channel disconnected, exiting loop.");
                        break;
                    }
                },
            }
        }        

        // if let Err(err) = client.unregister_symbol(NOTIFY_TAG) {
        //     error!("Failed to unregister symbol: {} ", err);
        // }
    }
    


    // Make sure the ADS client is closed.
    client.finalize();
        


    

    info!("Goodbye!");
    
}