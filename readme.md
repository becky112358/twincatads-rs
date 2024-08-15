# twincatads-rs

A rust wrapper for the TwinCAT ADS library provided by Beckhoff. Includes a convenient client class that makes connecting to a target trivial.
Uploads symbol and data type information from the target to easily read and write structures and arrays. With 0.3.2, we switched to Tokio channels for safe, asynchronous communication and easy integration into other projects.

See src/bin/commtest.rs for a complete usage example. Note that commtest is always changing to test in different applications.

## Description

When trying to connect via ADS on a system where TwinCAT XAR or XAE is already installed and running, the connections will fail. In those cases, you need to use the TcAdsDll to connect. This crate uses Bindgen to create a wrapper. Both 32- and 64-bit builds should now be supported.

### Known limitations

#### Function Blocks (FB) and Structures with references

Function blocks and structures containing reference variables can't be properly parsed. Individual items within those FBs and Structs can be parsed, but not
the item as a whole. 

Reading a complete structure is a design goal for the twincatads-rs. However, deserializing a complete function block is not currently a feature in use and isn't
currently targetted.

#### Strings

Reading a string that is not declared as T_MaxString using read_symbol_value is not currently supported. 
Use read_symbol_string_value instead.

Writing strings with write_symbol_value is similarly limited. write_symbol_value needs a type:

```
    if let Err(err) = client.write_symbol_value::<MaxString>(
        "GM.sTarget",
        MaxString::from_string("I'm not even supposed to be here today.")
    ) {
        println!("An error occurred writing <MaxString> for: {}", err);
    }
```

It's cleaner to simply use write_symbol_string_value:

```
    if let Err(err) = client.write_symbol_string_value(
        "GM.sTarget",
        "There is no spoon."
    ) {
        println!("An error occurred writing the tag: {}", err);
    }
```


Fixed strings are possible:
```
fixed_str: [u8; 32],
```

#### Structures

`read_symbol_value` and `write_symbol_value` need a known type with only fixed-width members, so nothing dynamic like `String` or `Vec`. If
a structure is predefined, it can be used with `read_symbol_value` and `write_symbol_value`.

Example:
```
use zerocopy::{AsBytes, FromBytes};

#[derive(FromBytes, AsBytes, Default, Debug)]
#[repr(C)]
struct MyStruct {
    field1: u32,
    field2: u64,
}

...

let st = MyStruct {
    field1 : 123,
    field2: 456
};

if let Err(err) = client.write_symbol_value::<MyStruct> ("GVL.myStructVariable", &st) {
    println!("You blew it!");
}

```

However, dynamic programs that don't know symbols or types until run-time don't always have the luxury of pre-defined, static types.
Instead, those values are stored in mechutil::VariantValue instances. Structure symbols registered for on data change notification
will automatically be parsed into a VariantValue::Object. To write a structure stored in a VariantValue::Object, use  `write_symbol_variant_value`.

The VariantValue::Object must have the exact same structure as the PLC. Structures in the target don't always byte-align the same
as they do in the local client; the adsclient handles adjusting the alignment of the data.


## Prerequisites

### For building:
- The TwinCAT XAE running on 32- or 64-bit Microsoft Windows is required. You wouldn't use this library in any other situation, anyway.
- - We use version 3.1.4024.XX
- - The TwinCAT XAE is a free download from beckhoff.com.
- Bindgen requires LLVM. See "Building" below.

### For running
- The TwinCAT XAE, XAR or even just the ADS Router (TC1000) will suffice.

## Installation

- Add the dependency to Cargo.toml

`twincatads-rs = "0.3"`



## Building

The bindings to Bindgen requires LLVM. 

With earlier versions of Bindgen, we needed to have LLVM installed specifically. You can download the latest and greatest of LLVM here:
https://releases.llvm.org/download.html

Find LLVM-XX.XX.XX-win64.exe on the releases page. Before building, make sure to have set the LIBCLANG_PATH.

Example: 
`SET LIBCLANG_PATH=c:\Program Files\LLVM\bin`


However, we have not needed this with more current versions of Bindgen, although Bindgen's documention still lists
the requirement. Our advice is to install as needed.

- Build with cargo

To build matching the default target of your machine (usually 64-bit):

`cargo build`

To specifically build a 64-bit version:
`cargo build --target x86_64-pc-windows-msvc`


To build a 32-bit version:

`cargo build --target i686-pc-windows-msvc`




# Demo

For a more complete example, see src/bin/commtest.rs.


Using the recommended client library, including channels for notifications.

```
    let (mut client, rx) = AdsClient::new();
    // Supply AMS Address. If not set, localhost is used.
    client.set_address("192.168.127.1.1.1");
    
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
    

    const NOTIFY_TAG :&str = "GM.fReal";
    
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
        
```


Simple example using the Beckhoff API directly.

```
use std::{time};
use twincatads_rs::*;




pub fn hello_ads() {
    println!("Hello, ADS!");

    unsafe {
        let val = AdsGetDllVersion();

        println!("The ADS Dll version is {}", val);

        let client_port = AdsPortOpen();

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

        std::thread::sleep(time::Duration::from_millis(1000));


        AdsPortClose();

        println!("Goodbye...");

    }
}
```



# License
MIT

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the “Software”), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.


# Disclaimers
Myself and my company are not related to Beckhoff.

THE SOFTWARE IS PROVIDED “AS IS”, WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.