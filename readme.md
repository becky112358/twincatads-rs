# twincatads-rs

A rust wrapper for the TwinCAT ADS library provided by Beckhoff.

## Description

When trying to connect via ADS on a system where TwinCAT XAR or XAE is already installed and running, the connections will fail. In those cases, you need to use the TcAdsDll to connect. This crate uses Bindgen to create a wrapper. Both 32- and 64-bit builds should now be supported.

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

`twincatads-rs = "0.2"`



## Building

The bindings to Bindgen requires LLVM. 

With earlier versions of Bindgen, we needed to have LLVM installed specifically. You can download the latest and greatest of LLVM here:
https://releases.llvm.org/download.html

Find LLVM-XX.XX.XX-win64.exe on the releases page. Before building, make sure to have set the LIBCLANG_PATH.

Example: 
`SET LIBCLANG_PATH=c:\Program Files\LLVM\bin`


However, we have not needed this with more current versions of Bindgen, although Bindgen's documention still lists
the requirement. My advice is to install as needed.

- Build with cargo

To build matching the default target of your machine (usually 64-bit):

`cargo build`

To specifically build a 64-bit version:
`cargo build --target x86_64-pc-windows-msvc`


To build a 32-bit version:

`cargo build --target i686-pc-windows-msvc`




# Demo

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