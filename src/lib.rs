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

use node_builtins::BUILTINS;
use serde_json::Value;
use std::default::Default;
use std::error::Error as StdError;
use std::fmt;
use std::fs::File;
use std::io::{Error as IOError, ErrorKind as IOErrorKind};
use std::path::{Component as PathComponent, Path, PathBuf};

static ROOT: &str = "/";

#[derive(Debug)]
pub enum Error {
    /// Failed to parse a package.json file.
    JSONError(serde_json::Error),
    /// Could not read a file.
    IOError(IOError),
    /// A Basedir was not configured.
    UnconfiguredBasedir,
    /// Something else went wrong.
    ResolutionError(ResolutionError),
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Error {
        Error::JSONError(err)
    }
}
impl From<IOError> for Error {
    fn from(err: IOError) -> Error {
        Error::IOError(err)
    }
}
impl From<ResolutionError> for Error {
    fn from(err: ResolutionError) -> Error {
        Error::ResolutionError(err)
    }
}

/// An Error, returned when the module could not be resolved.
#[derive(Debug)]
pub struct ResolutionError {
    description: String,
}
impl ResolutionError {
    fn new(description: &str) -> Self {
        ResolutionError {
            description: String::from(description),
        }
    }
}

impl fmt::Display for ResolutionError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.description)
    }
}

impl StdError for ResolutionError {
    fn description(&self) -> &str {
        self.description.as_str()
    }
    fn cause(&self) -> Option<&StdError> {
        None
    }
}

/// Resolver instances keep track of options.
#[derive(Clone)]
pub struct Resolver {
    basedir: Option<PathBuf>,
    extensions: Vec<String>,
    preserve_symlinks: bool,
    main_fields: Vec<String>,
}

impl Default for Resolver {
    /// Create a new resolver with the default Node.js configuration.
    ///
    /// - It resolves .js, .json, and .node files, in that order;
    /// - It expands symlinks;
    /// - It uses the package.json "main" field for bare specifier lookups.
    fn default() -> Resolver {
        Resolver {
            basedir: None,
            extensions: vec![
                String::from(".js"),
                String::from(".json"),
                String::from(".node"),
            ],
            preserve_symlinks: false,
            main_fields: vec![String::from("main")],
        }
    }
}

impl Resolver {
    #[deprecated(since = "2.3.0", note = "use Resolver::default() instead")]
    pub fn new() -> Self {
        Resolver::default()
    }

    fn get_basedir(&self) -> Result<&Path, Error> {
        self.basedir
            .as_ref()
            .ok_or_else(|| Error::UnconfiguredBasedir)
            .map(PathBuf::as_path)
    }

    /// Create a new resolver with a different basedir.
    pub fn with_basedir(&self, basedir: PathBuf) -> Self {
        Resolver {
            basedir: Some(basedir),
            ..self.clone()
        }
    }

    /// Use a different set of extensions. Consumes the Resolver instance.
    /// The default is `&[".js", ".json", ".node"]`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use node_resolve::Resolver;
    ///
    /// assert_eq!(Ok(PathBuf::from("./fixtures/module/index.mjs")),
    ///     Resolver::default()
    ///         .extensions(&[".mjs", ".js", ".json"])
    ///         .with_basedir("./fixtures")
    ///         .resolve("./module")
    /// );
    /// ```
    pub fn extensions<T>(self, extensions: T) -> Self
    where
        T: IntoIterator,
        T::Item: ToString,
    {
        Resolver {
            extensions: normalize_extensions(extensions),
            ..self
        }
    }

    /// Use a different set of main fields. Consumes the Resolver instance.
    /// The default is `&["main"]`.
    ///
    /// Main fields are used to determine the entry point of a folder with a
    /// `package.json` file. Each main field is tried in order, and the value
    /// of the first one that exists is used as the path to the entry point of
    /// the folder.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use node_resolve::Resolver;
    ///
    /// assert_eq!(Ok(PathBuf::from("./fixtures/module-main/main.mjs"),
    ///     Resolver::default()
    ///         .extensions(&[".mjs", ".js", ".json"])
    ///         .main_fields(&["module", "main"])
    ///         .with_basedir("./fixtures")
    ///         .resolve("./module-main")
    /// );
    /// ```
    pub fn main_fields<T>(self, main_fields: T) -> Self
    where
        T: IntoIterator,
        T::Item: ToString,
    {
        Resolver {
            main_fields: main_fields
                .into_iter()
                .map(|field| field.to_string())
                .collect(),
            ..self
        }
    }

    /// Configure whether symlinks should be preserved. Consumes the Resolver instance.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use node_resolve::Resolver;
    ///
    /// assert_eq!(Ok(PathBuf::from("./fixtures/symlink/node_modules/dep/main.js").canonicalize()),
    ///     Resolver::default()
    ///            .preserve_symlinks(true)
    ///            .with_basedir(PathBuf::from("./fixtures/symlink"))
    ///            .resolve("dep")
    /// );
    /// ```
    ///
    /// ```rust
    /// use node_resolve::Resolver;
    ///
    /// assert_eq!(Ok(PathBuf::from("./fixtures/symlink/linked/main.js").canonicalize()),
    ///     Resolver::default()
    ///            .preserve_symlinks(false)
    ///            .with_basedir(PathBuf::from("./fixtures/symlink"))
    ///            .resolve("dep")
    /// };
    /// ```
    pub fn preserve_symlinks(self, preserve_symlinks: bool) -> Self {
        Resolver {
            preserve_symlinks,
            ..self
        }
    }

    /// Resolve a `require('target')` argument.
    pub fn resolve(&self, target: &str) -> Result<PathBuf, Error> {
        // 1. If X is a core module
        if is_core_module(target) {
            // 1.a. Return the core module
            return Ok(PathBuf::from(target));
        }

        // 2. If X begins with '/'
        let basedir = if target.starts_with('/') {
            // 2.a. Set Y to be the filesystem root
            Path::new(ROOT)
        } else {
            self.get_basedir()?
        };

        // 3. If X begins with './' or '/' or '../'
        if target.starts_with("./") || target.starts_with('/') || target.starts_with("../") {
            let path = basedir.join(target);
            return self
                .resolve_as_file(&path)
                .or_else(|_| self.resolve_as_directory(&path))
                .and_then(|p| self.normalize(&p));
        }

        self.resolve_node_modules(target)
            .and_then(|p| self.normalize(&p))
    }

    /// Normalize a path to a module. If symlinks should be preserved, this only removes
    /// unnecessary `./`s and `../`s from the path. Else it does `realpath()`.
    fn normalize(&self, path: &Path) -> Result<PathBuf, Error> {
        if self.preserve_symlinks {
            Ok(normalize_path(path))
        } else {
            path.canonicalize().map_err(Into::into)
        }
    }

    /// Resolve a path as a file. If `path` refers to a file, it is returned;
    /// otherwise the `path` + each extension is tried.
    fn resolve_as_file(&self, path: &Path) -> Result<PathBuf, Error> {
        // 1. If X is a file, load X as JavaScript text.
        if path.is_file() {
            return Ok(path.to_path_buf());
        }

        // 1. If X.js is a file, load X.js as JavaScript text.
        // 2. If X.json is a file, parse X.json to a JavaScript object.
        // 3. If X.node is a file, load X.node as binary addon.
        let str_path = path
            .to_str()
            .ok_or_else(|| Error::ResolutionError(ResolutionError::new("Invalid path")))?;
        for ext in &self.extensions {
            let ext_path = PathBuf::from(format!("{}{}", str_path, ext));
            if ext_path.is_file() {
                return Ok(ext_path);
            }
        }

        Err(IOError::new(IOErrorKind::NotFound, "Not Found").into())
    }

    /// Resolve a path as a directory, using the "main" key from a package.json file if it
    /// exists, or resolving to the index.EXT file if it exists.
    fn resolve_as_directory(&self, path: &Path) -> Result<PathBuf, Error> {
        if !path.is_dir() {
            return Err(IOError::new(IOErrorKind::NotFound, "Not Found").into());
        }

        // 1. If X/package.json is a file, use it.
        let pkg_path = path.join("package.json");
        if pkg_path.is_file() {
            let main = self.resolve_package_main(&pkg_path);
            if main.is_ok() {
                return main;
            }
        }

        // 2. LOAD_INDEX(X)
        self.resolve_index(path)
    }

    /// Resolve using the package.json "main" key.
    fn resolve_package_main(&self, pkg_path: &Path) -> Result<PathBuf, Error> {
        let pkg_dir = pkg_path.parent().unwrap_or_else(|| Path::new(ROOT));
        let file = File::open(pkg_path)?;
        let pkg: Value = serde_json::from_reader(file)?;
        if !pkg.is_object() {
            return Err(ResolutionError::new("package.json is not an object").into());
        }

        let main_field = self
            .main_fields
            .iter()
            .find(|name| pkg[name].is_string())
            .and_then(|name| pkg[name].as_str());
        match main_field {
            Some(target) => {
                let path = pkg_dir.join(target);
                self.resolve_as_file(&path)
                    .or_else(|_| self.resolve_as_directory(&path))
            }
            None => {
                Err(ResolutionError::new("package.json does not contain a \"main\" string").into())
            }
        }
    }

    /// Resolve a directory to its index.EXT.
    fn resolve_index(&self, path: &Path) -> Result<PathBuf, Error> {
        // 1. If X/index.js is a file, load X/index.js as JavaScript text.
        // 2. If X/index.json is a file, parse X/index.json to a JavaScript object.
        // 3. If X/index.node is a file, load X/index.node as binary addon.
        for ext in self.extensions.iter() {
            let ext_path = path.join(format!("index{}", ext));
            if ext_path.is_file() {
                return Ok(ext_path);
            }
        }

        Err(Error::IOError(IOError::new(
            IOErrorKind::NotFound,
            "Not Found",
        )))
    }

    /// Resolve by walking up node_modules folders.
    fn resolve_node_modules(&self, target: &str) -> Result<PathBuf, Error> {
        let basedir = self.get_basedir()?;
        let node_modules = basedir.join("node_modules");
        if node_modules.is_dir() {
            let path = node_modules.join(target);
            let result = self
                .resolve_as_file(&path)
                .or_else(|_| self.resolve_as_directory(&path));
            if result.is_ok() {
                return result;
            }
        }

        match basedir.parent() {
            Some(parent) => self
                .with_basedir(parent.to_path_buf())
                .resolve_node_modules(target),
            None => Err(Error::IOError(IOError::new(
                IOErrorKind::NotFound,
                "Not Found",
            ))),
        }
    }
}

/// Remove excess components like `/./` and `/../` from a `Path`.
fn normalize_path(p: &Path) -> PathBuf {
    let mut normalized = PathBuf::from("/");
    for part in p.components() {
        match part {
            PathComponent::Prefix(ref prefix) => {
                normalized.push(prefix.as_os_str());
            }
            PathComponent::RootDir => {
                normalized.push("/");
            }
            PathComponent::ParentDir => {
                normalized.pop();
            }
            PathComponent::CurDir => {
                // Nothing
            }
            PathComponent::Normal(name) => {
                normalized.push(name);
            }
        }
    }
    normalized
}

fn normalize_extensions<T>(extensions: T) -> Vec<String>
where
    T: IntoIterator,
    T::Item: ToString,
{
    extensions
        .into_iter()
        .map(|ext| ext.to_string())
        .map(|ext| {
            if ext.starts_with('.') {
                ext
            } else {
                format!(".{}", ext)
            }
        })
        .collect()
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
pub fn resolve(target: &str) -> Result<PathBuf, Error> {
    Resolver::default()
        .with_basedir(PathBuf::from("."))
        .resolve(target)
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
pub fn resolve_from(target: &str, basedir: PathBuf) -> Result<PathBuf, Error> {
    Resolver::default().with_basedir(basedir).resolve(target)
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::PathBuf;
    use super::*;

    fn fixture(part: &str) -> PathBuf {
        env::current_dir().unwrap().join("fixtures").join(part)
    }
    fn resolve_fixture(target: &str) -> PathBuf {
        resolve_from(target, fixture("")).unwrap()
    }

    #[test]
    fn appends_extensions() {
        assert_eq!(
            fixture("extensions/js-file.js"),
            resolve_fixture("./extensions/js-file")
        );
        assert_eq!(
            fixture("extensions/json-file.json"),
            resolve_fixture("./extensions/json-file")
        );
        assert_eq!(
            fixture("extensions/native-file.node"),
            resolve_fixture("./extensions/native-file")
        );
        assert_eq!(
            fixture("extensions/other-file.ext"),
            resolve_fixture("./extensions/other-file.ext")
        );
        assert_eq!(
            fixture("extensions/no-ext"),
            resolve_fixture("./extensions/no-ext")
        );
        assert_eq!(
            fixture("extensions/other-file.ext"),
            Resolver::default()
                .extensions(&[".ext"])
                .with_basedir(fixture(""))
                .resolve("./extensions/other-file")
                .unwrap()
        );
        assert_eq!(
            fixture("extensions/module.mjs"),
            Resolver::default()
                .extensions(&[".mjs"])
                .with_basedir(fixture(""))
                .resolve("./extensions/module")
                .unwrap()
        );
    }

    #[test]
    fn resolves_package_json() {
        assert_eq!(
            fixture("package-json/main-file/whatever.js"),
            resolve_fixture("./package-json/main-file")
        );
        assert_eq!(
            fixture("package-json/main-file-noext/whatever.js"),
            resolve_fixture("./package-json/main-file-noext")
        );
        assert_eq!(
            fixture("package-json/main-dir/subdir/index.js"),
            resolve_fixture("./package-json/main-dir")
        );
        assert_eq!(
            fixture("package-json/not-object/index.js"),
            resolve_fixture("./package-json/not-object")
        );
        assert_eq!(
            fixture("package-json/invalid/index.js"),
            resolve_fixture("./package-json/invalid")
        );
        assert_eq!(
            fixture("package-json/main-none/index.js"),
            resolve_fixture("./package-json/main-none")
        );
        assert_eq!(
            fixture("package-json/main-file/whatever.js"),
            Resolver::default()
                .main_fields(&["module", "main"])
                .with_basedir(fixture(""))
                .resolve("./package-json/main-file")
                .unwrap()
        );
        assert_eq!(
            fixture("package-json/module/index.mjs"),
            Resolver::default()
                .extensions(&[".mjs", ".js"])
                .main_fields(&["module", "main"])
                .with_basedir(fixture(""))
                .resolve("./package-json/module")
                .unwrap()
        );
        assert_eq!(
            fixture("package-json/module-main/main.mjs"),
            Resolver::default()
                .extensions(&[".mjs", ".js"])
                .main_fields(&["module", "main"])
                .with_basedir(fixture(""))
                .resolve("./package-json/module-main")
                .unwrap()
        );
    }

    #[test]
    fn resolves_node_modules() {
        assert_eq!(
            fixture("node-modules/same-dir/node_modules/a.js"),
            resolve_from("a", fixture("node-modules/same-dir")).unwrap()
        );
        assert_eq!(
            fixture("node-modules/parent-dir/node_modules/a/index.js"),
            resolve_from("a", fixture("node-modules/parent-dir/src")).unwrap()
        );
        assert_eq!(
            fixture("node-modules/package-json/node_modules/dep/lib/index.js"),
            resolve_from("dep", fixture("node-modules/package-json")).unwrap()
        );
        assert_eq!(
            fixture("node-modules/walk/src/node_modules/not-ok/index.js"),
            resolve_from("not-ok", fixture("node-modules/walk/src")).unwrap()
        );
        assert_eq!(
            fixture("node-modules/walk/node_modules/ok/index.js"),
            resolve_from("ok", fixture("node-modules/walk/src")).unwrap()
        );
    }

    #[test]
    fn preserves_symlinks() {
        assert_eq!(
            fixture("symlink/node_modules/dep/main.js"),
            Resolver::default()
                .preserve_symlinks(true)
                .with_basedir(fixture("symlink"))
                .resolve("dep")
                .unwrap()
        );
    }

    #[test]
    fn does_not_preserve_symlinks() {
        assert_eq!(
            fixture("symlink/linked/main.js"),
            Resolver::default()
                .preserve_symlinks(false)
                .with_basedir(fixture("symlink"))
                .resolve("dep")
                .unwrap()
        );
    }

    #[test]
    fn resolves_absolute_specifier() {
        let full_path = fixture("extensions/js-file");
        let id = full_path.to_str().unwrap();
        assert_eq!(fixture("extensions/js-file.js"), resolve(id).unwrap());
    }

    #[test]
    fn core_modules() {
        assert!(is_core_module("events"));
        assert!(!is_core_module("events/"));
        assert!(!is_core_module("./events"));
        assert!(is_core_module("stream"));
        assert!(!is_core_module("acorn"));
    }
}
