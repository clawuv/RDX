#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    rd_protocol_fuzz::exercise(data);
});
