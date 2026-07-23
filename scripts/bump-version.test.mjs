// Self-test for bump-version.mjs — the riskiest guard script (regex surgery on
// TOML/lock files). Run via `node --test scripts/*.test.mjs`.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  SEMVER_RE,
  setCargoTomlVersion,
  findCargoTomlVersion,
  setCargoLockVersion,
  findCargoLockVersion,
} from "./bump-version.mjs";

test("SEMVER_RE accepts valid versions (incl. pre-release/build)", () => {
  for (const v of [
    "1.2.3",
    "0.5.1",
    "10.20.30",
    "0.0.0",
    "1.2.3-beta.1",
    "1.0.0+build.5",
    "1.2.3-rc.1+sha.abc",
  ]) {
    assert.ok(SEMVER_RE.test(v), `should accept ${v}`);
  }
});

test("SEMVER_RE rejects leading zeros and malformed versions (D1)", () => {
  for (const v of [
    "01.2.3",
    "1.02.3",
    "1.2.03",
    "1.2",
    "1.2.3.4",
    "v1.2.3",
    "a.b.c",
    "",
    "1.2.x",
  ]) {
    assert.ok(!SEMVER_RE.test(v), `should reject ${v}`);
  }
});

test("setCargoTomlVersion rewrites [package] version only, not dependencies", () => {
  const toml = [
    "[package]",
    'name = "portreaper"',
    'version = "0.5.1"',
    'edition = "2021"',
    "",
    "[dependencies]",
    'serde = "1.0.200"',
  ].join("\n");
  const out = setCargoTomlVersion(toml, "0.6.0");
  assert.equal(findCargoTomlVersion(out), "0.6.0");
  assert.ok(out.includes('serde = "1.0.200"'), "dependency version untouched");
  assert.ok(!out.includes('version = "0.5.1"'), "old package version gone");
});

test("setCargoTomlVersion ignores a version in an earlier dependency table", () => {
  const toml = [
    "[dependencies]",
    'foo = { version = "1.2.3" }',
    "",
    "[package]",
    'version = "0.5.1"',
  ].join("\n");
  const out = setCargoTomlVersion(toml, "0.6.0");
  assert.ok(out.includes('foo = { version = "1.2.3" }'), "dependency untouched");
  assert.equal(findCargoTomlVersion(out), "0.6.0");
});

test("setCargoLockVersion targets the portreaper block, not portreaper_lib/serde", () => {
  const lock = [
    "[[package]]",
    'name = "portreaper_lib"',
    'version = "0.5.1"',
    "",
    "[[package]]",
    'name = "portreaper"',
    'version = "0.5.1"',
    "dependencies = [",
    ' "portreaper_lib",',
    "]",
    "",
    "[[package]]",
    'name = "serde"',
    'version = "1.0.200"',
  ].join("\n");
  const out = setCargoLockVersion(lock, "0.6.0");
  assert.equal(findCargoLockVersion(out), "0.6.0");
  assert.ok(
    out.includes('name = "portreaper_lib"\nversion = "0.5.1"'),
    "portreaper_lib block untouched (exact-name match)",
  );
  assert.ok(out.includes('name = "serde"\nversion = "1.0.200"'), "serde block untouched");
});
