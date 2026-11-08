// Compile-time check of the Rust mirror (src/fmxabi.rs) against the Claris
// SDK headers. Run scripts/check-abi.sh; needs sdk/PlugInSDK (not committed).
#include "FMWrapper/FMXTypes.h"
#include "FMWrapper/FMXText.h"
#include "FMWrapper/FMXData.h"
#include "FMWrapper/FMXExtern.h"
#include "FMWrapper/FMXCalcEngine.h"
#include <cstddef>
static_assert(sizeof(FMX_ExternCallStruct) == 74, "struct size");
static_assert(offsetof(FMX_ExternCallStruct, unusedID) == 2, "unusedID");
static_assert(offsetof(FMX_ExternCallStruct, entryPoint) == 6, "entryPoint");
static_assert(offsetof(FMX_ExternCallStruct, whichCall) == 15, "whichCall");
static_assert(offsetof(FMX_ExternCallStruct, unsafeCalls) == 16, "unsafeCalls");
static_assert(offsetof(FMX_ExternCallStruct, parm1) == 17, "parm1");
static_assert(offsetof(FMX_ExternCallStruct, parm2) == 18, "parm2");
static_assert(offsetof(FMX_ExternCallStruct, result) == 42, "result");
static_assert(offsetof(FMX_ExternCallStruct, cCurrentEnv) == 66, "cCurrentEnv");
static_assert(kFMXT_Init == 0 && kFMXT_Idle == 1 && kFMXT_Shutdown == 4 && kFMXT_GetString == 7, "calls");
static_assert(kFMXT_NameStr == 128 && kFMXT_AppConfigStr == 129 && kFMXT_OptionsStr == 131, "strings");
static_assert(kFMXT_UserIdle == 0 && kFMXT_ScriptRunning == 3 && kFMXT_Unsafe == 4, "idle levels");
static_assert(kCurrentExtnVersion == 77, "SDK 26.0.3 API version");
static_assert(fmx::ExprEnv::kDisplayInAllDialogs == 0xFF00 && fmx::ExprEnv::kMacCompatible == 2, "flags");
static_assert(fmx::ExprEnv::kGet_FileName == 1005 && fmx::ExprEnv::kGet_LayoutName == 1010 &&
              fmx::ExprEnv::kGet_ScriptName == 1026 && fmx::ExprEnv::kGet_AccountName == 1032 &&
              fmx::ExprEnv::kGet_WindowName == 1048 && fmx::ExprEnv::kGet_LayoutTableName == 1061 &&
              fmx::ExprEnv::kGet_HostName == 1008 && fmx::ExprEnv::kGet_UserName == 1031 &&
              fmx::ExprEnv::kGet_WindowMode == 1013, "Get ids");
static_assert(fmx::Data::kDTText == 1 && fmx::Locale::kType_System == 1, "data/locale");
int main() { return 0; }
