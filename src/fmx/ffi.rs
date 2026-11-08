//! C exports of FMWrapper.framework that we use.
//!
//! Signatures from the `extern "C"` blocks of the Claris SDK headers
//! (FMXText.h, FMXData.h, FMXCalcEngine.h, FMXTypes.h). C++ references are
//! pointers at the ABI level. Every call takes a trailing `_fmxcpt&` that
//! FileMaker fills when the C++ side threw.
//! All symbols are present in FileMaker Pro 26.0.3's FMWrapper (checked with
//! `nm -gU`), and `cargo build --features fmx` links against it.

#![allow(non_snake_case, dead_code)]

use core::ffi::c_void;

pub type Text = c_void;
pub type Data = c_void;
pub type DataVect = c_void;
pub type ExprEnv = c_void;
pub type QuadChar = c_void;
pub type Locale = c_void;

/// fmx::_fmxcpt { int32 m_vers = 1; int32 m_code = kNoErr; }
#[repr(C)]
pub struct Fmxcpt {
    pub m_vers: i32,
    pub m_code: i32,
}
impl Fmxcpt {
    pub fn new() -> Self {
        Fmxcpt { m_vers: 1, m_code: 0 }
    }
}

/// fmx::ExtPluginType
pub type ExtPluginType =
    unsafe extern "C" fn(function_id: i16, env: *const ExprEnv, parms: *const DataVect, result: *mut Data) -> i16;

#[link(name = "FMWrapper", kind = "framework")]
extern "C" {
    // Text
    pub fn FM_Text_Constructor1(x: *mut Fmxcpt) -> *mut Text;
    pub fn FM_Text_Delete(this: *mut Text, x: *mut Fmxcpt);
    pub fn FM_Text_GetSize(this: *const Text, x: *mut Fmxcpt) -> u32;
    pub fn FM_Text_AssignUnicodeWithLength(this: *mut Text, s: *const u16, len: u32, x: *mut Fmxcpt);
    pub fn FM_Text_GetUnicode(this: *const Text, s: *mut u16, position: u32, size: u32, x: *mut Fmxcpt);

    // Locale
    pub fn FM_Locale_Constructor1(input_type: i32, x: *mut Fmxcpt) -> *mut Locale;
    pub fn FM_Locale_Delete(this: *mut Locale, x: *mut Fmxcpt);

    // Data
    pub fn FM_Data_Constructor1(x: *mut Fmxcpt) -> *mut Data;
    pub fn FM_Data_Delete(this: *mut Data, x: *mut Fmxcpt);
    pub fn FM_Data_GetAsText(this: *const Data, x: *mut Fmxcpt) -> *const Text;
    pub fn FM_Data_SetAsText(this: *mut Data, text: *const Text, locale: *const Locale, native_type: i32, x: *mut Fmxcpt) -> i16;

    // DataVect
    pub fn FM_DataVect_Size(this: *const DataVect, x: *mut Fmxcpt) -> u32;
    pub fn FM_DataVect_AtAsText(this: *const DataVect, position: u32, x: *mut Fmxcpt) -> *const Text;

    // QuadChar
    pub fn FM_QuadChar_Constructor2(c0: i8, c1: i8, c2: i8, c3: i8, x: *mut Fmxcpt) -> *mut QuadChar;
    pub fn FM_QuadChar_Delete(this: *mut QuadChar, x: *mut Fmxcpt);

    // ExprEnv
    pub fn FM_ExprEnv_Constructor1(x: *mut Fmxcpt) -> *mut ExprEnv;
    pub fn FM_ExprEnv_Delete(this: *mut ExprEnv, x: *mut Fmxcpt);
    pub fn FM_ExprEnv_EvaluateGetFunction(this: *const ExprEnv, function_value: i16, result: *mut Data, x: *mut Fmxcpt) -> i16;
    pub fn FM_ExprEnv_Evaluate(this: *const ExprEnv, expression: *const Text, result: *mut Data, x: *mut Fmxcpt) -> i16;
    pub fn FM_ExprEnv_RegisterExternalFunctionEx(
        plugin_id: *const QuadChar,
        function_id: i16,
        name: *const Text,
        prototype: *const Text,
        description: *const Text,
        min_args: i16,
        max_args: i16,
        compatible_on_flags: u32,
        func: ExtPluginType,
        x: *mut Fmxcpt,
    ) -> i16;
    pub fn FM_ExprEnv_UnRegisterExternalFunction(plugin_id: *const QuadChar, function_id: i16, x: *mut Fmxcpt) -> i16;
}
