use lapdev_common::kube::{KubeContainerImage, KubeContainerInfo};
use lapdev_rpc::error::ApiError;

pub fn validate_containers(containers: &[KubeContainerInfo]) -> Result<(), ApiError> {
    if containers.is_empty() {
        return Err(ApiError::InvalidRequest(
            "At least one container is required".to_string(),
        ));
    }

    for (index, container) in containers.iter().enumerate() {
        if container.name.trim().is_empty() {
            return Err(ApiError::InvalidRequest(format!(
                "Container {} name cannot be empty",
                index + 1
            )));
        }

        match &container.image {
            KubeContainerImage::Custom(image) => {
                if image.trim().is_empty() {
                    return Err(ApiError::InvalidRequest(format!(
                        "Container '{}' custom image cannot be empty",
                        container.name
                    )));
                }
            }
            KubeContainerImage::FollowOriginal => {
                // No validation needed for FollowOriginal
            }
        }

        // Validate CPU resources
        if let Some(cpu_request) = &container.cpu_request {
            if !is_valid_cpu_quantity(cpu_request) {
                return Err(ApiError::InvalidRequest(format!("Container '{}' has invalid CPU request format. Use formats like '100m', '0.1', '1'. Minimum precision is 1m (0.001 CPU)", container.name)));
            }
        }

        if let Some(cpu_limit) = &container.cpu_limit {
            if !is_valid_cpu_quantity(cpu_limit) {
                return Err(ApiError::InvalidRequest(format!("Container '{}' has invalid CPU limit format. Use formats like '100m', '0.1', '1'. Minimum precision is 1m (0.001 CPU)", container.name)));
            }
        }

        // Validate memory resources
        if let Some(memory_request) = &container.memory_request {
            if !is_valid_memory_quantity(memory_request) {
                return Err(ApiError::InvalidRequest(format!("Container '{}' has invalid memory request format. Use formats like '128Mi', '1Gi', '512M'. Maximum 3 decimal places allowed", container.name)));
            }
        }

        if let Some(memory_limit) = &container.memory_limit {
            if !is_valid_memory_quantity(memory_limit) {
                return Err(ApiError::InvalidRequest(format!("Container '{}' has invalid memory limit format. Use formats like '128Mi', '1Gi', '512M'. Maximum 3 decimal places allowed", container.name)));
            }
        }

        // Validate environment variables
        for env_var in &container.env_vars {
            if env_var.name.trim().is_empty() {
                return Err(ApiError::InvalidRequest(format!(
                    "Container '{}' has environment variable with empty name",
                    container.name
                )));
            }
        }
    }

    Ok(())
}

pub fn is_valid_cpu_quantity(quantity: &str) -> bool {
    // CPU validation for Kubernetes
    // Supports formats like: 100m, 0.1, 1, 2.5, etc.
    // Kubernetes minimum precision is 1m (0.001 CPU)
    if quantity.trim().is_empty() {
        return false; // Empty values are invalid - must specify a resource amount
    }

    let quantity = quantity.trim();

    // CPU can have 'm' suffix for millicores or be a plain decimal number
    let re = match regex::Regex::new(r"^(\d+\.?\d*|\.\d+)m?$") {
        Ok(regex) => regex,
        Err(_) => return false,
    };

    if !re.is_match(quantity) {
        return false;
    }

    // Parse the numeric part and validate precision constraints
    let (numeric_part, has_millicore_suffix) = if quantity.ends_with('m') {
        (&quantity[..quantity.len() - 1], true)
    } else {
        (quantity, false)
    };

    if let Ok(value) = numeric_part.parse::<f64>() {
        if value <= 0.0 {
            return false; // Must be positive
        }

        if has_millicore_suffix {
            // For millicores (m suffix), minimum is 1m, so value must be >= 1.0
            value >= 1.0
        } else {
            // For plain decimal CPU values, minimum precision is 0.001 (1m equivalent)
            value >= 0.001
        }
    } else {
        false
    }
}

pub fn is_valid_memory_quantity(quantity: &str) -> bool {
    // Memory validation for Kubernetes
    // Supports formats like: 128Mi, 1Gi, 512M, 1000000000, etc.
    // Maximum precision is 3 decimal places
    if quantity.trim().is_empty() {
        return false; // Empty values are invalid - must specify a resource amount
    }

    let quantity = quantity.trim();

    // Memory can have binary (Ki, Mi, Gi, Ti, Pi, Ei) or decimal (K, M, G, T, P, E) suffixes
    let valid_suffixes = [
        "Ki", "Mi", "Gi", "Ti", "Pi", "Ei", "K", "M", "G", "T", "P", "E",
    ];

    let (numeric_part, _suffix) =
        if let Some(suffix) = valid_suffixes.iter().find(|&s| quantity.ends_with(s)) {
            (&quantity[..quantity.len() - suffix.len()], Some(*suffix))
        } else {
            // No suffix means it's in bytes
            (quantity, None)
        };

    // Validate the numeric part is a positive number with max 3 decimal places
    if let Ok(value) = numeric_part.parse::<f64>() {
        if value <= 0.0 {
            return false; // Must be positive
        }

        // Check decimal places constraint (max 3 decimal places)
        if let Some(decimal_pos) = numeric_part.find('.') {
            let decimal_part = &numeric_part[decimal_pos + 1..];
            if decimal_part.len() > 3 {
                return false; // More than 3 decimal places
            }
        }

        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_validation() {
        // Valid CPU values
        assert!(is_valid_cpu_quantity("100m")); // 100 millicores
        assert!(is_valid_cpu_quantity("1m")); // 1 millicore (minimum)
        assert!(is_valid_cpu_quantity("0.1")); // 0.1 CPU (100m equivalent)
        assert!(is_valid_cpu_quantity("1")); // 1 CPU
        assert!(is_valid_cpu_quantity("2.5")); // 2.5 CPUs
        assert!(is_valid_cpu_quantity("500m")); // 500 millicores
        assert!(is_valid_cpu_quantity("1.5m")); // 1.5 millicores
        assert!(is_valid_cpu_quantity("0.001")); // Minimum decimal precision

        // Invalid CPU values
        assert!(!is_valid_cpu_quantity("")); // Empty
        assert!(!is_valid_cpu_quantity("   ")); // Whitespace
        assert!(!is_valid_cpu_quantity("0.5m")); // Below 1m minimum
        assert!(!is_valid_cpu_quantity("0.0005")); // Below 0.001 minimum
        assert!(!is_valid_cpu_quantity("0m")); // Zero millicores
        assert!(!is_valid_cpu_quantity("0")); // Zero CPU
        assert!(!is_valid_cpu_quantity("100Mi")); // Wrong suffix
        assert!(!is_valid_cpu_quantity("-100m")); // Negative
        assert!(!is_valid_cpu_quantity("abc")); // Non-numeric
        assert!(!is_valid_cpu_quantity("100x")); // Invalid suffix
    }

    #[test]
    fn test_memory_validation() {
        // Valid memory values
        assert!(is_valid_memory_quantity("128Mi"));
        assert!(is_valid_memory_quantity("1Gi"));
        assert!(is_valid_memory_quantity("512M"));
        assert!(is_valid_memory_quantity("1000000000")); // Raw bytes
        assert!(is_valid_memory_quantity("2Ti"));
        assert!(is_valid_memory_quantity("1.5")); // 1 decimal place
        assert!(is_valid_memory_quantity("1.5Gi")); // 1 decimal place
        assert!(is_valid_memory_quantity("128.25Mi")); // 2 decimal places
        assert!(is_valid_memory_quantity("1.125Gi")); // 3 decimal places (max)
        assert!(is_valid_memory_quantity("0.5Gi")); // Decimal with suffix

        // Invalid memory values
        assert!(!is_valid_memory_quantity("")); // Empty
        assert!(!is_valid_memory_quantity("   ")); // Whitespace
        assert!(!is_valid_memory_quantity("100m")); // CPU suffix on memory
        assert!(!is_valid_memory_quantity("-128Mi")); // Negative
        assert!(!is_valid_memory_quantity("abc")); // Non-numeric
        assert!(!is_valid_memory_quantity("100x")); // Invalid suffix
        assert!(!is_valid_memory_quantity("0Mi")); // Zero
        assert!(!is_valid_memory_quantity("1.1234Gi")); // 4 decimal places (too many)
        assert!(!is_valid_memory_quantity("1.1234")); // 4 decimal places (too many)
        assert!(!is_valid_memory_quantity("128.12345Mi")); // 5 decimal places (too many)
        assert!(!is_valid_memory_quantity("128.12345")); // 5 decimal places (too many)
    }
}
