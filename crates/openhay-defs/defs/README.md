# Bundled defs

`ph-4.0.0.zinc` is the official Project Haystack 4.0.0 defs namespace (libs
`ph`, `phScience`, `phIoT`, `phIct`, 719 defs), downloaded unmodified from
<https://project-haystack.org/download/defs.zinc>.

Project Haystack's defs are published under the Academic Free License 3.0
(<https://project-haystack.org/license>). They are data, bundled as-is; the
rest of openhay is BSD-3-Clause.

To update: download the new `defs.zinc`, save it as `ph-<version>.zinc`,
and change the `include_str!` in `src/lib.rs` and the version test.
