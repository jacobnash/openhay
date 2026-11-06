# haxall-derived conformance vectors

`testHaystack-<version>.json` holds test **cases** derived from the
[haxall](https://github.com/haxall/haxall) reference implementation's test suite
(`src/test/testHaystack/fan`: FilterTest, ZincTest, TrioTest) at the tag named
inside the file.

- **Inputs:** the Zinc grids, Zinc tags, Trio documents and filter strings in
  those tests, plus the FilterTest record sets written as Zinc tags.
- **Expected values:** produced by running every input through haxall's own
  `haystack` pod at the same version (decoded values as Hayson, errors, and the
  records each filter matches). Nobody transcribed them by hand. The FilterTest
  expectations were cross-checked against the test file's own assertions, and
  all 92 agree.

The generator is in `tools/haxall-vectors/` in the openhay repo.

haxall is Copyright (c) SkyFoundry and licensed under the Academic Free License
3.0. These vectors are distributed under the same license, separately from
openhay's own BSD-3-Clause code. No haxall source code is included.
