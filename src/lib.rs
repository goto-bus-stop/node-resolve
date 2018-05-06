//! Resolve module identifiers in a Node-style `require()` to a full file path.
//!
//! ```rust
//! use node_resolve::{resolve, resolve_from};
//!
//! resolve("abc");
//! // → Ok("/path/to/cwd/node_modules/abc/index.js")
//! resolve_from("abc", PathBuf::from("/other/path"));
//! // → Ok("/other/path/node_modules/abc/index.js")
//! ```

extern crate serde_json;
extern crate node_builtins;

use std::fmt;
use std::fs::File;
use std::error::Error;
use std::path::PathBuf;
use serde_json::Value;
use node_builtins::BUILTINS;

/// An Error, returned when the module could not be resolved.
#[derive(Debug)]
pub struct ResolutionError {
    description: String
}
impl ResolutionError {
    fn new(description: &str) -> Self {
        ResolutionError { description: String::from(description) }
    }
}

impl From<serde_json::Error> for ResolutionError {
    fn from(_error: serde_json::Error) -> Self {
        ResolutionError::new("Json parse error")
    }
}

impl From<std::io::Error> for ResolutionError {
    fn from(_error: std::io::Error) -> Self {
        ResolutionError::new("Io error")
    }
}

impl fmt::Display for ResolutionError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.description)
    }
}

impl Error for ResolutionError {
    fn description(&self) -> &str {
        self.description.as_str()
    }
    fn cause(&self) -> Option<&Error> {
        None
    }
}

/// Resolver instances keep track of options.
#[derive(Clone)]
pub struct Resolver {
    basedir: Option<PathBuf>,
    extensions: Vec<String>,
    preserve_symlinks: bool,
}

impl Resolver {
    /// Create a new resolver with the given options.
    pub fn new() -> Self {
        Resolver {
            basedir: None,
            extensions: vec![
                String::from(".js"),
                String::from(".json"),
                String::from(".node"),
            ],
            preserve_symlinks: false,
        }
    }

    fn get_basedir(&self) -> Result<&PathBuf, ResolutionError> {
        self.basedir.as_ref().ok_or_else(|| ResolutionError::new("Must set a basedir before resolving"))
    }

    /// Create a new resolver with a different basedir.
    pub fn with_basedir(&self, basedir: PathBuf) -> Self {
        Resolver { basedir: Some(basedir), ..self.clone() }
    }

    /// Create a new resolver with a different set of extensions.
    pub fn with_extensions<T>(&self, extensions: T) -> Self
        where T: IntoIterator,
              T::Item: ToString
    {
        Resolver {
            extensions: extensions.into_iter()
                .map(|ext| ext.to_string())
                .map(|ext| if ext.starts_with('.') {
                    ext
                } else {
                    format!(".{}", ext)
                })
                .collect(),
            ..self.clone()
        }
    }

    /// Create a new resolver with a different symlink option.
    pub fn preserve_symlinks(&self, preserve_symlinks: bool) -> Self {
        Resolver { preserve_symlinks, ..self.clone() }
    }

    /// Resolve a `require()` argument.
    pub fn resolve(&self, target: &str) -> Result<PathBuf, ResolutionError> {
        // 1. If X is a core module
        if is_core_module(&target) {
            // 1.a. Return the core module
            return Ok(PathBuf::from(target));
        }

        // TODO how to not always initialise this here?
        let root = PathBuf::from("/");
        // 2. If X begins with '/'
        let basedir = if target.starts_with('/') {
            // 2.a. Set Y to be the filesystem root
            &root
        } else {
            self.get_basedir()?
        };

        // 3. If X begins with './' or '/' or '../'
        if target.starts_with("./") || target.starts_with('/') || target.starts_with("../") {
            let path = basedir.as_path().join(target);
            return self.resolve_as_file(&path)
                .or_else(|_| self.resolve_as_directory(&path));
        }

        self.resolve_node_modules(&target)
    }

    /// Resolve a path as a file. If `path` refers to a file, it is returned;
    /// otherwise the `path` + each extension is tried.
    fn resolve_as_file(&self, path: &PathBuf) -> Result<PathBuf, ResolutionError> {
        // 1. If X is a file, load X as JavaScript text.
        if path.is_file() {
            return Ok(path.clone());
        }

        // 1. If X.js is a file, load X.js as JavaScript text.
        // 2. If X.json is a file, parse X.json to a JavaScript object.
        // 3. If X.node is a file, load X.node as binary addon.
        let str_path = path.to_str().ok_or_else(|| ResolutionError::new("Invalid path"))?;
        for ext in &self.extensions {
            let ext_path = PathBuf::from(format!("{}{}", str_path, ext));
            if ext_path.is_file() {
                return Ok(ext_path);
            }
        }

        Err(ResolutionError::new("Not found"))
    }

    /// Resolve a path as a directory, using the "main" key from a package.json file if it
    /// exists, or resolving to the index.EXT file if it exists.
    fn resolve_as_directory(&self, path: &PathBuf) -> Result<PathBuf, ResolutionError> {
        // 1. If X/package.json is a file, use it.
        let pkg_path = path.join("package.json");
        if pkg_path.is_file() {
            let main = self.resolve_package_main(&pkg_path);
            if main.is_ok() {
                return main
            }
        }

        // 2. LOAD_INDEX(X)
        self.resolve_index(path)
    }

    /// Resolve using the package.json "main" key.
    fn resolve_package_main(&self, pkg_path: &PathBuf) -> Result<PathBuf, ResolutionError> {
        // TODO how to not always initialise this here?
        let root = PathBuf::from("/");
        let pkg_dir = pkg_path.parent().unwrap_or(&root);
        let file = File::open(pkg_path)?;
        let pkg: Value = serde_json::from_reader(file)?;
        if !pkg.is_object() {
            return Err(ResolutionError::new("package.json is not an object"));
        }

        match pkg["main"].as_str() {
            Some(target) => {
                let path = pkg_dir.join(target);
                self.resolve_as_file(&path)
                    .or_else(|_| self.resolve_as_directory(&path))
            },
            None => Err(ResolutionError::new("package.json does not contain a \"main\" string"))
        }
    }

    /// Resolve a directory to its index.EXT.
    fn resolve_index(&self, path: &PathBuf) -> Result<PathBuf, ResolutionError> {
        // 1. If X/index.js is a file, load X/index.js as JavaScript text.
        // 2. If X/index.json is a file, parse X/index.json to a JavaScript object.
        // 3. If X/index.node is a file, load X/index.node as binary addon.
        for ext in &self.extensions {
            let ext_path = path.join(format!("index{}", ext));
            if ext_path.is_file() {
                return Ok(ext_path);
            }
        }

        Err(ResolutionError::new("Not found"))
    }

    /// Resolve by walking up node_modules folders.
    fn resolve_node_modules(&self, target: &str) -> Result<PathBuf, ResolutionError> {
        let basedir = self.get_basedir()?;
        let node_modules = basedir.join("node_modules");
        if node_modules.is_dir() {
            let path = node_modules.join(target);
            let result = self.resolve_as_file(&path)
                .or_else(|_| self.resolve_as_directory(&path));
            if result.is_ok() {
                return result
            }
        }

        match basedir.parent() {
            Some(parent) => self.with_basedir(parent.to_path_buf()).resolve_node_modules(target),
            None => Err(ResolutionError::new("Not found")),
        }
    }
}

/// Check if a string references a core module, such as "events".
pub fn is_core_module(target: &str) -> bool {
    BUILTINS.iter().any(|builtin| builtin == &target)
}

/// Resolve a node.js module path relative to the current working directory.
/// Returns the absolute path to the module, or an error.
///
/// ```rust
/// match resolve("./lib") {
///     Ok(path) => println!("Path is: {:?}", path),
///     Err(err) => panic!("Failed: {:?}", err),
/// }
/// ```
pub fn resolve(target: &str) -> Result<PathBuf, ResolutionError> {
    Resolver::new().with_basedir(PathBuf::from(".")).resolve(target)
}

/// Resolve a node.js module path relative to `basedir`.
/// Returns the absolute path to the module, or an error.
///
/// ```rust
/// match resolve_from("./index.js", env::current_dir().unwrap()) {
///     Ok(path) => println!("Path is: {:?}", path),
///     Err(err) => panic!("Failed: {:?}", err),
/// }
/// ```
pub fn resolve_from(target: &str, basedir: PathBuf) -> Result<PathBuf, ResolutionError> {
    Resolver::new().with_basedir(basedir).resolve(target)
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::PathBuf;

    fn fixture(part: &str) -> PathBuf {
        env::current_dir().unwrap().join("fixtures").join(part)
    }
    fn resolve_fixture(target: &str) -> PathBuf {
        ::resolve_from(target, fixture("")).unwrap()
    }

    #[test]
    fn appends_extensions() {
        assert_eq!(fixture("extensions/js-file.js"), resolve_fixture("./extensions/js-file"));
        assert_eq!(fixture("extensions/json-file.json"), resolve_fixture("./extensions/json-file"));
        assert_eq!(fixture("extensions/native-file.node"), resolve_fixture("./extensions/native-file"));
        assert_eq!(fixture("extensions/other-file.ext"), resolve_fixture("./extensions/other-file.ext"));
        assert_eq!(fixture("extensions/no-ext"), resolve_fixture("./extensions/no-ext"));
    }

    #[test]
    fn resolves_package_json() {
        assert_eq!(fixture("package-json/main-file/whatever.js"), resolve_fixture("./package-json/main-file"));
        assert_eq!(fixture("package-json/main-file-noext/whatever.js"), resolve_fixture("./package-json/main-file-noext"));
        assert_eq!(fixture("package-json/main-dir/subdir/index.js"), resolve_fixture("./package-json/main-dir"));
        assert_eq!(fixture("package-json/not-object/index.js"), resolve_fixture("./package-json/not-object"));
        assert_eq!(fixture("package-json/invalid/index.js"), resolve_fixture("./package-json/invalid"));
        assert_eq!(fixture("package-json/main-none/index.js"), resolve_fixture("./package-json/main-none"));
    }

    #[test]
    fn resolves_node_modules() {
        assert_eq!(fixture("node-modules/same-dir/node_modules/a.js"), ::resolve_from("a", fixture("node-modules/same-dir")).unwrap());
        assert_eq!(fixture("node-modules/parent-dir/node_modules/a/index.js"), ::resolve_from("a", fixture("node-modules/parent-dir/src")).unwrap());
        assert_eq!(fixture("node-modules/package-json/node_modules/dep/lib/index.js"), ::resolve_from("dep", fixture("node-modules/package-json")).unwrap());
        assert_eq!(fixture("node-modules/walk/src/node_modules/not-ok/index.js"), ::resolve_from("not-ok", fixture("node-modules/walk/src")).unwrap());
        assert_eq!(fixture("node-modules/walk/node_modules/ok/index.js"), ::resolve_from("ok", fixture("node-modules/walk/src")).unwrap());
    }

    #[test]
    fn resolves_absolute_specifier() {
        let full_path = fixture("extensions/js-file");
        let id = full_path.to_str().unwrap();
        assert_eq!(fixture("extensions/js-file.js"), ::resolve(id).unwrap());
    }

    #[test]
    fn core_modules() {
        assert!(::is_core_module("events"));
        assert!(!::is_core_module("./events"));
        assert!(::is_core_module("stream"));
        assert!(!::is_core_module("acorn"));
    }
}
