use aws_sdk_ec2::config::Region as SdkRegion;

/// Macro to define AWS regions with their metadata in a single location.
/// This eliminates duplication - adding a new region requires only one line.
///
/// Usage:
/// ```
/// define_regions! {
///     VariantName => "aws-region-id" => "Human Readable Name",
///     ...
/// }
/// ```
macro_rules! define_regions {
    (
        $(
            $variant:ident => $id:expr => $name:expr
        ),* $(,)?
    ) => {
        /// Represents AWS regions supported by the application
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Region {
            $(
                $variant,
            )*
        }

        impl Region {
            /// Get all available regions
            pub fn all() -> Vec<Region> {
                vec![
                    $(
                        Region::$variant,
                    )*
                ]
            }

            /// Get the AWS SDK region string
            pub fn as_str(&self) -> &'static str {
                match self {
                    $(
                        Region::$variant => $id,
                    )*
                }
            }

            /// Get human-readable display name
            pub fn display_name(&self) -> &'static str {
                match self {
                    $(
                        Region::$variant => $name,
                    )*
                }
            }

            /// Convert to AWS SDK Region type
            pub fn to_sdk_region(&self) -> SdkRegion {
                SdkRegion::new(self.as_str().to_string())
            }

            /// Parse from AWS SDK region
            pub fn from_sdk_region(sdk_region: &SdkRegion) -> Option<Region> {
                Region::from_str(sdk_region.as_ref())
            }

            /// Parse from string
            pub fn from_str(s: &str) -> Option<Region> {
                match s {
                    $(
                        $id => Some(Region::$variant),
                    )*
                    _ => None,
                }
            }
        }
    };
}

// Single source of truth for all AWS regions
// To add a new region, simply add one line here:
// NewVariant => "aws-region-id" => "Human Readable Name",
define_regions! {
    UsEast1 => "us-east-1" => "US East (N. Virginia)",
    UsEast2 => "us-east-2" => "US East (Ohio)",
    UsWest1 => "us-west-1" => "US West (N. California)",
    UsWest2 => "us-west-2" => "US West (Oregon)",
    ApSoutheast1 => "ap-southeast-1" => "Asia Pacific (Singapore)",
    ApSoutheast2 => "ap-southeast-2" => "Asia Pacific (Sydney)",
    ApSoutheast4 => "ap-southeast-4" => "Asia Pacific (Melbourne)",
    ApSouthEast3 => "ap-southeast-3" => "Asia Pacific (Jakarta)",
    ApEast1 => "ap-east-1" => "Asia Pacific (Hong Kong)",
    ApNortheast1 => "ap-northeast-1" => "Asia Pacific (Tokyo)",
    ApNortheast2 => "ap-northeast-2" => "Asia Pacific (Seoul)",
    ApNortheast3 => "ap-northeast-3" => "Asia Pacific (Osaka)",
    ApSouth1 => "ap-south-1" => "Asia Pacific (Mumbai)",
    ApSouth2 => "ap-south-2" => "Asia Pacific (Hyderabad)",
    ApSoutheast5 => "ap-southeast-5" => "Asia Pacific (Malaysia)",
    ApSoutheast7 => "ap-southeast-7" => "Asia Pacific (Thailand)",
    EuWest1 => "eu-west-1" => "EU West (Ireland)",
    EuWest2 => "eu-west-2" => "EU West (London)",
    EuWest3 => "eu-west-3" => "EU West (Paris)",
    EuNorth1 => "eu-north-1" => "EU North (Stockholm)",
    EuCentral1 => "eu-central-1" => "EU Central (Frankfurt)",
    EuCentral2 => "eu-central-2" => "EU Central (Zurich)",
    EuSouth1 => "eu-south-1" => "EU South (Milan)",
    EuSouth2 => "eu-south-2" => "EU South (Spain)",
    CaCentral1 => "ca-central-1" => "Canada (Central)",
    CaWest1 => "ca-west-1" => "Canada West (Calgary)",
    AfSouth1 => "af-south-1" => "Africa (Cape Town)",
    MeSouth1 => "me-south-1" => "Middle East (Bahrain)",
    MeCentral1 => "me-central-1" => "Middle East (UAE)",
    IlCentral1 => "il-central-1" => "Israel (Tel Aviv)",
    MxCentral1 => "mx-central-1" => "Mexico (Central)",
    // CnNorth1 => "cn-north-1" => "China (Beijing)",
    // CnNorthwest1 => "cn-northwest-1" => "China (Ningxia)",
    SaEast1 => "sa-east-1" => "South America (São Paulo)",
}

/// Implement Display trait so we can use {} in format strings
impl std::fmt::Display for Region {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
