#Description
A rust wrapper for the TwinCAT ADS library provided by Beckhoff.

When trying to connect via ADS on a system where TwinCAT XAR or XAE is already installed and running, the connections will fail. In those cases, you need to use the TcAdsDll to connect. This crate used Bindgen to create a wrapper.

# Prerequisites
- TwinCAT running on 64-bit Microsoft Windows is required. You wouldn't use this library in any other situation, anyway.


# Building
- Bindgen requires LLVM. 

You can download the latest and greatest of LLVM here:
https://releases.llvm.org/download.html

Find LLVM-XX.XX.XX-win64.exe on the releases page.

- Before building, make sure to have set the LIBCLANG_PATH.

Example: 

`SET LIBCLANG_PATH=c:\Program Files\LLVM\bin`


- Build with cargo
`cargo build`



# License
MIT

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the “Software”), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.


# Disclaimers
Myself and my company are not related to Beckhoff.

THE SOFTWARE IS PROVIDED “AS IS”, WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.