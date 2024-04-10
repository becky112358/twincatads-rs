//
// Copyright (C) 2024 Automated Design Corp.. All Rights Reserved.
// Created Date: 2024-04-06 10:24:11
// -----
// Last Modified: 2024-04-10 14:48:33
// -----
// 
//

//! Quick communications test to evalutate operation of the library.
//! We include a test project in the source directory that can be
//! downloaded to a TC PLC instance for use in testing this
//! 

use std::thread;
use anyhow::{anyhow, Error};
use log::{info, error, debug};
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

    match client.read_symbol_value::<MaxString>("GM.sEcho") {
        Ok(val) => info!("STRING VALUE: {:?}", val.to_string()),
        Err(err) => error!("I failed to read the string tag: {}", err)
    }

    match client.read_symbol_value::<MaxString>("GK.sReadTest") {
        Ok(val) => info!("STRING VALUE: {:?}", val.to_string()),
        Err(err) => error!("I failed to read the string tag: {}", err)
    }    
    
    match client.read_symbol_string_value("GM.sEcho") {
        Ok(val) => info!("STRING VALUE: {:?}", val.to_string()),
        Err(err) => error!("I failed to read the string tag: {}", err)
    }    
    

    const NOTIFY_TAG :&str = "GM.aTestArray";
    
    if let Err(err) = client.register_symbol(NOTIFY_TAG) {
        error!("Failed to register symbol: {}", err);
    }
    else {
        let mut line = String::new();


        // On the parent context (e.g., main thread), listen for notifications
        thread::spawn(move || {

            //
            // When you call .iter() on a Receiver, you get an iterator that blocks waiting for new messages, 
            // and it continues until the channel is closed.
            //
            for notification in rx.iter() {
                println!("Notification received: {} : {}", notification.name, notification.value);
                // Handle the notification...
            }
        });


        std::io::stdin().read_line(&mut line).expect("Failed to read line");

        // if let Err(err) = client.unregister_symbol(NOTIFY_TAG) {
        //     error!("Failed to unregister symbol: {} ", err);
        // }
    }
    


    // Make sure the ADS client is closed.
    client.finalize();
        


    

    info!("Goodbye!");
    
}