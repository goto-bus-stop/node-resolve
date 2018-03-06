extern crate serde_json;

use std::env;
use std::fmt;
use std::fs::File;
use std::error::Error;
use std::path::PathBuf;
use serde_json::Value;

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
    fn from(error: serde_json::Error) -> Self {
        ResolutionError::new("Json parse error")
    }
}

impl From<std::io::Error> for ResolutionError {
    fn from(error: std::io::Error) -> Self {
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

#[derive(Clone)]
struct Resolver {
    basedir: PathBuf,
}

impl Resolver {
    fn create(basedir: PathBuf) -> Self {
        Resolver { basedir }
    }

    /// Create a new resolver with a different basedir.
    fn with_basedir(&self, basedir: PathBuf) -> Self {
        Resolver { basedir, ..self.clone() }
    }

    /// Check if a string references a core module, such as "events".
    fn is_core_module(&self, target: &String) -> bool {
        false
    }

    /// Resolve.
    fn resolve(&self, target: String) -> Result<PathBuf, ResolutionError> {
        // 1. If X is a core module
        if self.is_core_module(&target) {
            // 1.a. Return the core module
            return Ok(PathBuf::from(target));
        }

        // 2. If X begins with '/'
        if target.starts_with("/") {
            // 2.a. Set Y to be the filesystem root
            return self.with_basedir(PathBuf::from("/")).resolve(target);
        }

        // 3. If X begins with './' or '/' or '../'
        if target.starts_with("./") || target.starts_with("/") || target.starts_with("../") {
            let path = self.basedir.as_path().join(target);
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

        let str_path = path.to_str().ok_or_else(|| ResolutionError::new("Invalid path"))?;
        // 2. If X.js is a file, load X as JavaScript text.
        let js_path = PathBuf::from(format!("{}.js", str_path));
        if js_path.is_file() {
            return Ok(js_path);
        }

        // 3. If X.json is a file, parse X.json to a JavaScript Object.
        let json_path = PathBuf::from(format!("{}.json", str_path));
        if json_path.is_file() {
            return Ok(json_path);
        }

        // 4. If X.node is a file, load X.node as binary addon.
        let node_path = PathBuf::from(format!("{}.node", str_path));
        if node_path.is_file() {
            return Ok(node_path);
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
        println!("resolve_package_main: {:?}", pkg_path);
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
        println!("resolve_index: {:?}", path);
        // 1. If X/index.js is a file, load X/index.js as JavaScript text.
        let js_path = path.join("index.js");
        if js_path.is_file() {
            return Ok(js_path);
        }
        // 2. If X/index.json is a file, parse X/index.json to a JavaScript object.
        let json_path = path.join("index.json");
        if json_path.is_file() {
            return Ok(json_path);
        }
        // 3. If X/index.node is a file, load X/index.node as binary addon.
        let node_path = path.join("index.node");
        if node_path.is_file() {
            return Ok(node_path);
        }

        Err(ResolutionError::new("Not found"))
    }

    /// Resolve by walking up node_modules folders.
    fn resolve_node_modules(&self, target: &String) -> Result<PathBuf, ResolutionError> {
        Err(ResolutionError::new("Not implemented"))
    }
}

pub fn resolve(target: String) -> Result<PathBuf, ResolutionError> {
    env::current_dir()
        .map_err(|_| ResolutionError::new("Working directory does not exist"))
        .and_then(|dir| resolve_from(target, dir))
}
pub fn resolve_from(target: String, basedir: PathBuf) -> Result<PathBuf, ResolutionError> {
    Resolver::create(basedir).resolve(target)
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::PathBuf;

    fn fixture(part: &str) -> PathBuf {
        env::current_dir().unwrap().join("fixtures").join(part)
    }
    fn resolve_fixture(target: &str) -> PathBuf {
        ::resolve_from(String::from(target), fixture("")).unwrap()
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
        // assert_eq!(fixture("package-json/invalid/index.js"), resolve_fixture("./package-json/invalid"));
        assert_eq!(fixture("package-json/main-none/index.js"), resolve_fixture("./package-json/main-none"));
    }
}
