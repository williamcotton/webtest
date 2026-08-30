use std::fmt;

use crate::report::ExitClass;

#[derive(Debug)]
pub(crate) struct AppError {
    pub(crate) class: ExitClass,
    pub(crate) message: String,
}

impl AppError {
    pub(crate) fn usage(error: impl fmt::Display) -> Self {
        Self {
            class: ExitClass::Usage,
            message: error.to_string(),
        }
    }

    pub(crate) fn infrastructure(error: impl fmt::Display) -> Self {
        Self {
            class: ExitClass::Infrastructure,
            message: error.to_string(),
        }
    }

    pub(crate) fn internal(error: impl fmt::Display) -> Self {
        Self {
            class: ExitClass::Internal,
            message: error.to_string(),
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_preserve_exit_class_and_message() {
        for (error, class) in [
            (AppError::usage("usage"), ExitClass::Usage),
            (
                AppError::infrastructure("infrastructure"),
                ExitClass::Infrastructure,
            ),
            (AppError::internal("internal"), ExitClass::Internal),
        ] {
            assert_eq!(error.class, class);
            assert_eq!(error.to_string(), error.message);
        }
    }
}
