// (C) Copyright 2022 Automated Design Corp. All Rights Reserved.

//! This optional client module provides convenient wrappers for the API provided by the Beckhoff DLL.
//! Of most use is the adsclient module, which makes connecting to and interacting with
//! a PLC easy, providing the functionality required for most applications. 
//!  

pub mod client_types;
pub mod ads_data;
pub mod adsclient;
pub mod ads_symbol_loader;
