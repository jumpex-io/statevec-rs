// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use quote::quote;

pub(crate) fn expand_export_runtime_plugin(factory_expr: syn::Expr) -> proc_macro2::TokenStream {
    quote! {
        fn __statevec_runtime_plugin_factory() -> ::std::boxed::Box<dyn statevec_api::RuntimePluginFactory> {
            #factory_expr
        }

        // V1 metadata
        unsafe extern "C" fn __statevec_runtime_plugin_name_v1() -> statevec_api::RuntimeBytesRef {
            statevec_api::runtime_plugin_name_v1(__statevec_runtime_plugin_factory)
        }

        unsafe extern "C" fn __statevec_runtime_plugin_schema_bytes_v1() -> statevec_api::RuntimeBytesRef {
            static SCHEMA_BYTES: ::std::sync::OnceLock<::std::string::String> = ::std::sync::OnceLock::new();
            statevec_api::runtime_plugin_schema_bytes_v1(__statevec_runtime_plugin_factory, &SCHEMA_BYTES)
        }

        // V1 lifecycle
        unsafe extern "C" fn __statevec_runtime_plugin_create_runtime_v1(
            config: statevec_api::RuntimeBytesRef,
            out_runtime: *mut *mut ::std::ffi::c_void,
            out_error: *mut statevec_api::RuntimeErrorBuf,
        ) -> statevec_api::RuntimeCallStatus {
            unsafe {
                statevec_api::runtime_plugin_create_runtime_v1(
                    __statevec_runtime_plugin_factory,
                    config,
                    out_runtime,
                    out_error,
                )
            }
        }

        unsafe extern "C" fn __statevec_runtime_plugin_destroy_runtime_v1(runtime: *mut ::std::ffi::c_void) {
            unsafe {
                statevec_api::runtime_plugin_destroy_runtime_v1(runtime);
            }
        }

        unsafe extern "C" fn __statevec_runtime_plugin_on_unload_v1(
            runtime: *mut ::std::ffi::c_void,
            out_error: *mut statevec_api::RuntimeErrorBuf,
        ) -> statevec_api::RuntimeCallStatus {
            unsafe { statevec_api::runtime_plugin_on_unload_v1(runtime, out_error) }
        }

        // V1 execution
        unsafe extern "C" fn __statevec_runtime_plugin_run_tx_v1(
            runtime: *mut ::std::ffi::c_void,
            host: statevec_api::RuntimeHostContextV1,
            command: statevec_api::RuntimeCommandView,
            out_error: *mut statevec_api::RuntimeErrorBuf,
        ) -> statevec_api::RuntimeCallStatus {
            unsafe { statevec_api::runtime_plugin_run_tx_v1(runtime, host, command, out_error) }
        }

        unsafe extern "C" fn __statevec_runtime_plugin_validate_biz_invariants_v1(
            runtime: *mut ::std::ffi::c_void,
            host: statevec_api::RuntimeReadContextV1,
            out_error: *mut statevec_api::RuntimeErrorBuf,
        ) -> statevec_api::RuntimeCallStatus {
            unsafe {
                statevec_api::runtime_plugin_validate_biz_invariants_v1(runtime, host, out_error)
            }
        }

        // V1 entry
        #[unsafe(no_mangle)]
        pub extern "C" fn statevec_runtime_plugin_entry_v1(
        ) -> statevec_api::RuntimePluginApiV1 {
            statevec_api::RuntimePluginApiV1 {
                abi_version: statevec_api::RUNTIME_PLUGIN_ABI_VERSION_V1,
                plugin_name: __statevec_runtime_plugin_name_v1,
                schema_bytes: __statevec_runtime_plugin_schema_bytes_v1,
                create_runtime: __statevec_runtime_plugin_create_runtime_v1,
                destroy_runtime: __statevec_runtime_plugin_destroy_runtime_v1,
                run_tx: __statevec_runtime_plugin_run_tx_v1,
                validate_biz_invariants: __statevec_runtime_plugin_validate_biz_invariants_v1,
                on_unload: __statevec_runtime_plugin_on_unload_v1,
            }
        }
    }
}
