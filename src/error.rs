use std::io;
use std::error::Error as StdError;

quick_error! {
    #[derive(Debug)]
    pub enum Error {
        /// IO Error
        Io(err: io::Error) {
            from()
            cause(err)
        }
        /// Config error
        Config(err: ConfigError) {
            from()
            cause(err)
            display("Config error: {}", err.description())
        }
    }
}

quick_error! {
    #[derive(Debug)]
    pub enum ConfigError {
        /// IO Error
        Io(err: io::Error) {
            from()
            cause(err)
        }
        /// Config file not found.
        NotFound {
            from()
            description("not found")
        }
    }
}
