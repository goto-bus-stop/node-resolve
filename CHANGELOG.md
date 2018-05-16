# node-resolve change log

All notable changes to this project will be documented in this file.

This project adheres to [Semantic Versioning](http://semver.org/).

## 2.2.0
* Add `with_main_fields()` to configure the package.json `"main"` field name.
  ```rust
  Resolver::new()
    .with_main_fields(&["module", "main"])
    .with_extensions(&[".js", ".mjs", ".json"])
  ```
* Implement `Default` for `Resolver`.

## 2.1.1
* Exclude test symlink from the package so it can be published.

## 2.1.0
* Normalize paths before returning. You will now receive eg. `/a/b/c.js` instead
  of `/a/./b/c.js`.
* Implement `preserve_symlinks(bool)`. Symlinks are not resolved by default.
  This will change in the next major to match Node's behaviour.

## 2.0.0
* Take an `&str` argument instead of a `String`
* Expose `Resolver`

## 1.1.0
* Add `is_core_module()`

## 1.0.1
* Fix absolute specifiers like `require("/a")`

## 1.0.0
* Initial release.
