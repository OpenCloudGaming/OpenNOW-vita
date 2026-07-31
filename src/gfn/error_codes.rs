//! GFN error codes, ported from OpenNOW's gfnErrorCodeEnum.ts.
//! we had 3 substring classifiers before this and they all broke diferent on spanish text lol

/// error code as nvidia sends it. not an enum bc they add new codes without telling anyone
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GfnErrorCode(pub u32);

impl GfnErrorCode {
    pub const SUCCESS: Self = Self(15859712);
    pub const SESSION_SETUP_CANCELLED: Self = Self(15867905);
    pub const SESSION_SETUP_CANCELLED_DURING_QUEUING: Self = Self(15867906);
    pub const REQUEST_CANCELLED: Self = Self(15867907);
    pub const SYSTEM_SLEEP_DURING_SESSION_SETUP: Self = Self(15867909);
    pub const NO_INTERNET_DURING_SESSION_SETUP: Self = Self(15868417);
    pub const INVALID_OPERATION: Self = Self(3237085186);
    pub const NETWORK_ERROR: Self = Self(3237089282);
    pub const GET_ACTIVE_SESSION_SERVER_ERROR: Self = Self(3237089283);
    pub const AUTH_TOKEN_NOT_UPDATED: Self = Self(3237093377);
    pub const SESSION_FINISHED_STATE: Self = Self(3237093378);
    pub const RESPONSE_PARSE_FAILURE: Self = Self(3237093379);
    pub const INVALID_SERVER_RESPONSE: Self = Self(3237093381);
    pub const PUT_OR_POST_IN_PROGRESS: Self = Self(3237093382);
    pub const GRID_SERVER_NOT_INITIALIZED: Self = Self(3237093383);
    pub const D_O_M_EXCEPTION_IN_SESSION_CONTROL: Self = Self(3237093384);
    pub const INVALID_AD_STATE_TRANSITION: Self = Self(3237093386);
    pub const AUTH_TOKEN_UPDATE_TIMEOUT: Self = Self(3237093387);
    pub const SESSION_SERVER_ERROR_BEGIN: Self = Self(3237093632);
    /// CloudMatch `statusCode` 2.
    pub const REQUEST_FORBIDDEN: Self = Self(3237093634);
    /// CloudMatch `statusCode` 3.
    pub const SERVER_INTERNAL_TIMEOUT: Self = Self(3237093635);
    /// CloudMatch `statusCode` 4.
    pub const SERVER_INTERNAL_ERROR: Self = Self(3237093636);
    /// CloudMatch `statusCode` 5.
    pub const SERVER_INVALID_REQUEST: Self = Self(3237093637);
    /// CloudMatch `statusCode` 6.
    pub const SERVER_INVALID_REQUEST_VERSION: Self = Self(3237093638);
    /// CloudMatch `statusCode` 7.
    pub const SESSION_LIST_LIMIT_EXCEEDED: Self = Self(3237093639);
    /// CloudMatch `statusCode` 8.
    pub const INVALID_REQUEST_DATA_MALFORMED: Self = Self(3237093640);
    /// CloudMatch `statusCode` 9.
    pub const INVALID_REQUEST_DATA_MISSING: Self = Self(3237093641);
    /// CloudMatch `statusCode` 10.
    pub const REQUEST_LIMIT_EXCEEDED: Self = Self(3237093642);
    /// CloudMatch `statusCode` 11.
    pub const SESSION_LIMIT_EXCEEDED: Self = Self(3237093643);
    /// CloudMatch `statusCode` 12.
    pub const INVALID_REQUEST_VERSION_OUT_OF_DATE: Self = Self(3237093644);
    /// CloudMatch `statusCode` 13.
    pub const SESSION_ENTITLED_TIME_EXCEEDED: Self = Self(3237093645);
    /// CloudMatch `statusCode` 14.
    pub const AUTH_FAILURE: Self = Self(3237093646);
    /// CloudMatch `statusCode` 15.
    pub const INVALID_AUTHENTICATION_MALFORMED: Self = Self(3237093647);
    /// CloudMatch `statusCode` 16.
    pub const INVALID_AUTHENTICATION_EXPIRED: Self = Self(3237093648);
    /// CloudMatch `statusCode` 17.
    pub const INVALID_AUTHENTICATION_NOT_FOUND: Self = Self(3237093649);
    /// CloudMatch `statusCode` 18.
    pub const ENTITLEMENT_FAILURE: Self = Self(3237093650);
    /// CloudMatch `statusCode` 19.
    pub const INVALID_APP_ID_NOT_AVAILABLE: Self = Self(3237093651);
    /// CloudMatch `statusCode` 20.
    pub const INVALID_APP_ID_NOT_FOUND: Self = Self(3237093652);
    /// CloudMatch `statusCode` 21.
    pub const INVALID_SESSION_ID_MALFORMED: Self = Self(3237093653);
    /// CloudMatch `statusCode` 22.
    pub const INVALID_SESSION_ID_NOT_FOUND: Self = Self(3237093654);
    /// CloudMatch `statusCode` 23.
    pub const EULA_UN_ACCEPTED: Self = Self(3237093655);
    /// CloudMatch `statusCode` 24.
    pub const MAINTENANCE_STATUS: Self = Self(3237093656);
    /// CloudMatch `statusCode` 25.
    pub const SERVICE_UN_AVAILABLE: Self = Self(3237093657);
    /// CloudMatch `statusCode` 26.
    pub const STEAM_GUARD_REQUIRED: Self = Self(3237093658);
    /// CloudMatch `statusCode` 27.
    pub const STEAM_LOGIN_REQUIRED: Self = Self(3237093659);
    /// CloudMatch `statusCode` 28.
    pub const STEAM_GUARD_INVALID: Self = Self(3237093660);
    /// CloudMatch `statusCode` 29.
    pub const STEAM_PROFILE_PRIVATE: Self = Self(3237093661);
    /// CloudMatch `statusCode` 30.
    pub const INVALID_COUNTRY_CODE: Self = Self(3237093662);
    /// CloudMatch `statusCode` 31.
    pub const INVALID_LANGUAGE_CODE: Self = Self(3237093663);
    /// CloudMatch `statusCode` 32.
    pub const MISSING_COUNTRY_CODE: Self = Self(3237093664);
    /// CloudMatch `statusCode` 33.
    pub const MISSING_LANGUAGE_CODE: Self = Self(3237093665);
    /// CloudMatch `statusCode` 34.
    pub const SESSION_NOT_PAUSED: Self = Self(3237093666);
    /// CloudMatch `statusCode` 35.
    pub const EMAIL_NOT_VERIFIED: Self = Self(3237093667);
    /// CloudMatch `statusCode` 36.
    pub const INVALID_AUTHENTICATION_UNSUPPORTED_PROTOCOL: Self = Self(3237093668);
    /// CloudMatch `statusCode` 37.
    pub const INVALID_AUTHENTICATION_UNKNOWN_TOKEN: Self = Self(3237093669);
    /// CloudMatch `statusCode` 38.
    pub const INVALID_AUTHENTICATION_CREDENTIALS: Self = Self(3237093670);
    /// CloudMatch `statusCode` 39.
    pub const SESSION_NOT_PLAYING: Self = Self(3237093671);
    /// CloudMatch `statusCode` 40.
    pub const INVALID_SERVICE_RESPONSE: Self = Self(3237093672);
    /// CloudMatch `statusCode` 41.
    pub const APP_PATCHING: Self = Self(3237093673);
    /// CloudMatch `statusCode` 42.
    pub const GAME_NOT_FOUND: Self = Self(3237093674);
    /// CloudMatch `statusCode` 43.
    pub const NOT_ENOUGH_CREDITS: Self = Self(3237093675);
    /// CloudMatch `statusCode` 44.
    pub const INVITATION_ONLY_REGISTRATION: Self = Self(3237093676);
    /// CloudMatch `statusCode` 45.
    pub const REGION_NOT_SUPPORTED_FOR_REGISTRATION: Self = Self(3237093677);
    /// CloudMatch `statusCode` 46.
    pub const SESSION_TERMINATED_BY_ANOTHER_CLIENT: Self = Self(3237093678);
    /// CloudMatch `statusCode` 47.
    pub const DEVICE_ID_ALREADY_USED: Self = Self(3237093679);
    /// CloudMatch `statusCode` 48.
    pub const SERVICE_NOT_EXIST: Self = Self(3237093680);
    /// CloudMatch `statusCode` 49.
    pub const SESSION_EXPIRED: Self = Self(3237093681);
    /// CloudMatch `statusCode` 50.
    pub const SESSION_LIMIT_PER_DEVICE_REACHED: Self = Self(3237093682);
    /// CloudMatch `statusCode` 51.
    pub const FORWARDING_ZONE_OUT_OF_CAPACITY: Self = Self(3237093683);
    /// CloudMatch `statusCode` 52.
    pub const REGION_NOT_SUPPORTED_INDEFINITELY: Self = Self(3237093684);
    /// CloudMatch `statusCode` 53.
    pub const REGION_BANNED: Self = Self(3237093685);
    /// CloudMatch `statusCode` 54.
    pub const REGION_ON_HOLD_FOR_FREE: Self = Self(3237093686);
    /// CloudMatch `statusCode` 55.
    pub const REGION_ON_HOLD_FOR_PAID: Self = Self(3237093687);
    /// CloudMatch `statusCode` 56.
    pub const APP_MAINTENANCE_STATUS: Self = Self(3237093688);
    /// CloudMatch `statusCode` 57.
    pub const RESOURCE_POOL_NOT_CONFIGURED: Self = Self(3237093689);
    /// CloudMatch `statusCode` 58.
    pub const INSUFFICIENT_VM_CAPACITY: Self = Self(3237093690);
    /// CloudMatch `statusCode` 59.
    pub const INSUFFICIENT_ROUTE_CAPACITY: Self = Self(3237093691);
    /// CloudMatch `statusCode` 60.
    pub const INSUFFICIENT_SCRATCH_SPACE_CAPACITY: Self = Self(3237093692);
    /// CloudMatch `statusCode` 61.
    pub const REQUIRED_SEAT_INSTANCE_TYPE_NOT_SUPPORTED: Self = Self(3237093693);
    /// CloudMatch `statusCode` 62.
    pub const SERVER_SESSION_QUEUE_LENGTH_EXCEEDED: Self = Self(3237093694);
    /// CloudMatch `statusCode` 63.
    pub const REGION_NOT_SUPPORTED_FOR_STREAMING: Self = Self(3237093695);
    /// CloudMatch `statusCode` 64.
    pub const SESSION_FORWARD_REQUEST_ALLOCATION_TIME_EXPIRED: Self = Self(3237093696);
    /// CloudMatch `statusCode` 65.
    pub const SESSION_FORWARD_GAME_BINARIES_NOT_AVAILABLE: Self = Self(3237093697);
    /// CloudMatch `statusCode` 66.
    pub const GAME_BINARIES_NOT_AVAILABLE_IN_REGION: Self = Self(3237093698);
    /// CloudMatch `statusCode` 67.
    pub const UEK_RETRIEVAL_FAILED: Self = Self(3237093699);
    /// CloudMatch `statusCode` 68.
    pub const ENTITLEMENT_FAILURE_FOR_RESOURCE: Self = Self(3237093700);
    /// CloudMatch `statusCode` 69.
    pub const SESSION_IN_QUEUE_ABANDONED: Self = Self(3237093701);
    /// CloudMatch `statusCode` 70.
    pub const MEMBER_TERMINATED: Self = Self(3237093702);
    /// CloudMatch `statusCode` 71.
    pub const SESSION_REMOVED_FROM_QUEUE_MAINTENANCE: Self = Self(3237093703);
    /// CloudMatch `statusCode` 72.
    pub const ZONE_MAINTENANCE_STATUS: Self = Self(3237093704);
    /// CloudMatch `statusCode` 73.
    pub const GUEST_MODE_CAMPAIGN_DISABLED: Self = Self(3237093705);
    /// CloudMatch `statusCode` 74.
    pub const REGION_NOT_SUPPORTED_ANONYMOUS_ACCESS: Self = Self(3237093706);
    /// CloudMatch `statusCode` 75.
    pub const INSTANCE_TYPE_NOT_SUPPORTED_IN_SINGLE_REGION: Self = Self(3237093707);
    /// CloudMatch `statusCode` 78.
    pub const INVALID_ZONE_FOR_QUEUED_SESSION: Self = Self(3237093710);
    /// CloudMatch `statusCode` 79.
    pub const SESSION_WAITING_ADS_TIME_EXPIRED: Self = Self(3237093711);
    /// CloudMatch `statusCode` 80.
    pub const USER_CANCELLED_WATCHING_ADS: Self = Self(3237093712);
    /// CloudMatch `statusCode` 81.
    pub const STREAMING_NOT_ALLOWED_IN_LIMITED_MODE: Self = Self(3237093713);
    /// CloudMatch `statusCode` 82.
    pub const FORWARD_REQUEST_J_P_M_FAILED: Self = Self(3237093714);
    /// CloudMatch `statusCode` 83.
    pub const MAX_SESSION_NUMBER_LIMIT_EXCEEDED: Self = Self(3237093715);
    /// CloudMatch `statusCode` 84.
    pub const GUEST_MODE_PARTNER_CAPACITY_DISABLED: Self = Self(3237093716);
    /// CloudMatch `statusCode` 85.
    pub const SESSION_REJECTED_NO_CAPACITY: Self = Self(3237093717);
    /// CloudMatch `statusCode` 86.
    pub const SESSION_INSUFFICIENT_PLAYABILITY_LEVEL: Self = Self(3237093718);
    /// CloudMatch `statusCode` 87.
    pub const FORWARD_REQUEST_L_O_F_N_FAILED: Self = Self(3237093719);
    /// CloudMatch `statusCode` 88.
    pub const INVALID_TRANSPORT_REQUEST: Self = Self(3237093720);
    /// CloudMatch `statusCode` 89.
    pub const USER_STORAGE_NOT_AVAILABLE: Self = Self(3237093721);
    /// CloudMatch `statusCode` 90.
    pub const GFN_STORAGE_NOT_AVAILABLE: Self = Self(3237093722);
    /// CloudMatch `statusCode` 91.
    pub const APP_NOT_ALLOWED_TO_STREAM: Self = Self(3237093723);
    pub const SESSION_SERVER_ERROR_END: Self = Self(3237093887);
    pub const SOCKET_ERROR: Self = Self(3237101580);
    pub const ADDRESS_RESOLVE_FAILED: Self = Self(3237101581);
    pub const CONNECT_FAILED: Self = Self(3237101582);
    pub const SSL_ERROR: Self = Self(3237101583);
    pub const CONNECTION_TIMEOUT: Self = Self(3237101584);
    pub const DATA_RECEIVE_TIMEOUT: Self = Self(3237101585);
    pub const PEER_NO_RESPONSE: Self = Self(3237101586);
    pub const UNEXPECTED_HTTP_REDIRECT: Self = Self(3237101587);
    pub const DATA_SEND_FAILURE: Self = Self(3237101588);
    pub const DATA_RECEIVE_FAILURE: Self = Self(3237101589);
    pub const CERTIFICATE_REJECTED: Self = Self(3237101590);
    pub const DATA_NOT_ALLOWED: Self = Self(3237101591);
    pub const NETWORK_ERROR_UNKNOWN: Self = Self(3237101592);

    /// every code we know about, only used by tests to walk the table
    #[allow(dead_code)]
    pub const ALL: &'static [Self] = &[
        Self::SUCCESS,
        Self::SESSION_SETUP_CANCELLED,
        Self::SESSION_SETUP_CANCELLED_DURING_QUEUING,
        Self::REQUEST_CANCELLED,
        Self::SYSTEM_SLEEP_DURING_SESSION_SETUP,
        Self::NO_INTERNET_DURING_SESSION_SETUP,
        Self::INVALID_OPERATION,
        Self::NETWORK_ERROR,
        Self::GET_ACTIVE_SESSION_SERVER_ERROR,
        Self::AUTH_TOKEN_NOT_UPDATED,
        Self::SESSION_FINISHED_STATE,
        Self::RESPONSE_PARSE_FAILURE,
        Self::INVALID_SERVER_RESPONSE,
        Self::PUT_OR_POST_IN_PROGRESS,
        Self::GRID_SERVER_NOT_INITIALIZED,
        Self::D_O_M_EXCEPTION_IN_SESSION_CONTROL,
        Self::INVALID_AD_STATE_TRANSITION,
        Self::AUTH_TOKEN_UPDATE_TIMEOUT,
        Self::SESSION_SERVER_ERROR_BEGIN,
        Self::REQUEST_FORBIDDEN,
        Self::SERVER_INTERNAL_TIMEOUT,
        Self::SERVER_INTERNAL_ERROR,
        Self::SERVER_INVALID_REQUEST,
        Self::SERVER_INVALID_REQUEST_VERSION,
        Self::SESSION_LIST_LIMIT_EXCEEDED,
        Self::INVALID_REQUEST_DATA_MALFORMED,
        Self::INVALID_REQUEST_DATA_MISSING,
        Self::REQUEST_LIMIT_EXCEEDED,
        Self::SESSION_LIMIT_EXCEEDED,
        Self::INVALID_REQUEST_VERSION_OUT_OF_DATE,
        Self::SESSION_ENTITLED_TIME_EXCEEDED,
        Self::AUTH_FAILURE,
        Self::INVALID_AUTHENTICATION_MALFORMED,
        Self::INVALID_AUTHENTICATION_EXPIRED,
        Self::INVALID_AUTHENTICATION_NOT_FOUND,
        Self::ENTITLEMENT_FAILURE,
        Self::INVALID_APP_ID_NOT_AVAILABLE,
        Self::INVALID_APP_ID_NOT_FOUND,
        Self::INVALID_SESSION_ID_MALFORMED,
        Self::INVALID_SESSION_ID_NOT_FOUND,
        Self::EULA_UN_ACCEPTED,
        Self::MAINTENANCE_STATUS,
        Self::SERVICE_UN_AVAILABLE,
        Self::STEAM_GUARD_REQUIRED,
        Self::STEAM_LOGIN_REQUIRED,
        Self::STEAM_GUARD_INVALID,
        Self::STEAM_PROFILE_PRIVATE,
        Self::INVALID_COUNTRY_CODE,
        Self::INVALID_LANGUAGE_CODE,
        Self::MISSING_COUNTRY_CODE,
        Self::MISSING_LANGUAGE_CODE,
        Self::SESSION_NOT_PAUSED,
        Self::EMAIL_NOT_VERIFIED,
        Self::INVALID_AUTHENTICATION_UNSUPPORTED_PROTOCOL,
        Self::INVALID_AUTHENTICATION_UNKNOWN_TOKEN,
        Self::INVALID_AUTHENTICATION_CREDENTIALS,
        Self::SESSION_NOT_PLAYING,
        Self::INVALID_SERVICE_RESPONSE,
        Self::APP_PATCHING,
        Self::GAME_NOT_FOUND,
        Self::NOT_ENOUGH_CREDITS,
        Self::INVITATION_ONLY_REGISTRATION,
        Self::REGION_NOT_SUPPORTED_FOR_REGISTRATION,
        Self::SESSION_TERMINATED_BY_ANOTHER_CLIENT,
        Self::DEVICE_ID_ALREADY_USED,
        Self::SERVICE_NOT_EXIST,
        Self::SESSION_EXPIRED,
        Self::SESSION_LIMIT_PER_DEVICE_REACHED,
        Self::FORWARDING_ZONE_OUT_OF_CAPACITY,
        Self::REGION_NOT_SUPPORTED_INDEFINITELY,
        Self::REGION_BANNED,
        Self::REGION_ON_HOLD_FOR_FREE,
        Self::REGION_ON_HOLD_FOR_PAID,
        Self::APP_MAINTENANCE_STATUS,
        Self::RESOURCE_POOL_NOT_CONFIGURED,
        Self::INSUFFICIENT_VM_CAPACITY,
        Self::INSUFFICIENT_ROUTE_CAPACITY,
        Self::INSUFFICIENT_SCRATCH_SPACE_CAPACITY,
        Self::REQUIRED_SEAT_INSTANCE_TYPE_NOT_SUPPORTED,
        Self::SERVER_SESSION_QUEUE_LENGTH_EXCEEDED,
        Self::REGION_NOT_SUPPORTED_FOR_STREAMING,
        Self::SESSION_FORWARD_REQUEST_ALLOCATION_TIME_EXPIRED,
        Self::SESSION_FORWARD_GAME_BINARIES_NOT_AVAILABLE,
        Self::GAME_BINARIES_NOT_AVAILABLE_IN_REGION,
        Self::UEK_RETRIEVAL_FAILED,
        Self::ENTITLEMENT_FAILURE_FOR_RESOURCE,
        Self::SESSION_IN_QUEUE_ABANDONED,
        Self::MEMBER_TERMINATED,
        Self::SESSION_REMOVED_FROM_QUEUE_MAINTENANCE,
        Self::ZONE_MAINTENANCE_STATUS,
        Self::GUEST_MODE_CAMPAIGN_DISABLED,
        Self::REGION_NOT_SUPPORTED_ANONYMOUS_ACCESS,
        Self::INSTANCE_TYPE_NOT_SUPPORTED_IN_SINGLE_REGION,
        Self::INVALID_ZONE_FOR_QUEUED_SESSION,
        Self::SESSION_WAITING_ADS_TIME_EXPIRED,
        Self::USER_CANCELLED_WATCHING_ADS,
        Self::STREAMING_NOT_ALLOWED_IN_LIMITED_MODE,
        Self::FORWARD_REQUEST_J_P_M_FAILED,
        Self::MAX_SESSION_NUMBER_LIMIT_EXCEEDED,
        Self::GUEST_MODE_PARTNER_CAPACITY_DISABLED,
        Self::SESSION_REJECTED_NO_CAPACITY,
        Self::SESSION_INSUFFICIENT_PLAYABILITY_LEVEL,
        Self::FORWARD_REQUEST_L_O_F_N_FAILED,
        Self::INVALID_TRANSPORT_REQUEST,
        Self::USER_STORAGE_NOT_AVAILABLE,
        Self::GFN_STORAGE_NOT_AVAILABLE,
        Self::APP_NOT_ALLOWED_TO_STREAM,
        Self::SESSION_SERVER_ERROR_END,
        Self::SOCKET_ERROR,
        Self::ADDRESS_RESOLVE_FAILED,
        Self::CONNECT_FAILED,
        Self::SSL_ERROR,
        Self::CONNECTION_TIMEOUT,
        Self::DATA_RECEIVE_TIMEOUT,
        Self::PEER_NO_RESPONSE,
        Self::UNEXPECTED_HTTP_REDIRECT,
        Self::DATA_SEND_FAILURE,
        Self::DATA_RECEIVE_FAILURE,
        Self::CERTIFICATE_REJECTED,
        Self::DATA_NOT_ALLOWED,
        Self::NETWORK_ERROR_UNKNOWN,
    ];

    /// Server codes are this plus the `statusCode` CloudMatch reported.
    const SERVER_ERROR_BASE: u32 = 3237093632;

    // mirrors OpenNOW's computeErrorCode. unifiedErrorCode only wins over the derived code
    // when the derived one is one of the 3 generic ones, otherwise we'd lose a specific error
    pub fn from_cloudmatch(status_code: u32, unified_error_code: Option<i64>) -> Self {
        let mut code = Self::SESSION_SERVER_ERROR_BEGIN;
        if status_code == 1 {
            code = Self::SUCCESS;
        } else if status_code > 0 && status_code < 255 {
            code = Self(Self::SERVER_ERROR_BASE + status_code);
        }

        if let Some(unified) = unified_error_code
            && matches!(
                code,
                Self::SESSION_SERVER_ERROR_BEGIN
                    | Self::SERVER_INTERNAL_ERROR
                    | Self::INVALID_SERVER_RESPONSE
            )
            && let Ok(unified) = u32::try_from(unified)
        {
            code = Self(unified);
        }

        code
    }

    // last resort when theres no real code, patterns copied from OpenNOW's fallback
    pub fn from_description(description: &str) -> Option<Self> {
        let text = description.to_ascii_uppercase();
        Some(match () {
            _ if text.contains("INSUFFICIENT_PLAYABILITY") => {
                Self::SESSION_INSUFFICIENT_PLAYABILITY_LEVEL
            }
            _ if text.contains("SESSION_LIMIT") => Self::SESSION_LIMIT_EXCEEDED,
            _ if text.contains("MAINTENANCE") => Self::MAINTENANCE_STATUS,
            _ if text.contains("CAPACITY") || text.contains("QUEUE") => {
                Self::INSUFFICIENT_VM_CAPACITY
            }
            _ if text.contains("ENTITLEMENT") => Self::ENTITLEMENT_FAILURE,
            // check this last, AUTH shows up in unrelated descriptions too often
            _ if text.contains("AUTH") || text.contains("TOKEN") => Self::AUTH_FAILURE,
            _ => return None,
        })
    }

    // 404 left out on purpose, could mean "no such session" or just a bad url, not worth guessing
    pub fn from_http_status(status: u16) -> Option<Self> {
        Some(match status {
            401 => Self::AUTH_FAILURE,
            403 => Self::REQUEST_FORBIDDEN,
            429 => Self::REQUEST_LIMIT_EXCEEDED,
            500..=599 => Self::SERVER_INTERNAL_ERROR,
            _ => return None,
        })
    }

    // name for logging, None if we dont recognize the code yet
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::SUCCESS => "Success",
            Self::SESSION_SETUP_CANCELLED => "SessionSetupCancelled",
            Self::SESSION_SETUP_CANCELLED_DURING_QUEUING => "SessionSetupCancelledDuringQueuing",
            Self::REQUEST_CANCELLED => "RequestCancelled",
            Self::SYSTEM_SLEEP_DURING_SESSION_SETUP => "SystemSleepDuringSessionSetup",
            Self::NO_INTERNET_DURING_SESSION_SETUP => "NoInternetDuringSessionSetup",
            Self::INVALID_OPERATION => "InvalidOperation",
            Self::NETWORK_ERROR => "NetworkError",
            Self::GET_ACTIVE_SESSION_SERVER_ERROR => "GetActiveSessionServerError",
            Self::AUTH_TOKEN_NOT_UPDATED => "AuthTokenNotUpdated",
            Self::SESSION_FINISHED_STATE => "SessionFinishedState",
            Self::RESPONSE_PARSE_FAILURE => "ResponseParseFailure",
            Self::INVALID_SERVER_RESPONSE => "InvalidServerResponse",
            Self::PUT_OR_POST_IN_PROGRESS => "PutOrPostInProgress",
            Self::GRID_SERVER_NOT_INITIALIZED => "GridServerNotInitialized",
            Self::D_O_M_EXCEPTION_IN_SESSION_CONTROL => "DOMExceptionInSessionControl",
            Self::INVALID_AD_STATE_TRANSITION => "InvalidAdStateTransition",
            Self::AUTH_TOKEN_UPDATE_TIMEOUT => "AuthTokenUpdateTimeout",
            Self::SESSION_SERVER_ERROR_BEGIN => "SessionServerErrorBegin",
            Self::REQUEST_FORBIDDEN => "RequestForbidden",
            Self::SERVER_INTERNAL_TIMEOUT => "ServerInternalTimeout",
            Self::SERVER_INTERNAL_ERROR => "ServerInternalError",
            Self::SERVER_INVALID_REQUEST => "ServerInvalidRequest",
            Self::SERVER_INVALID_REQUEST_VERSION => "ServerInvalidRequestVersion",
            Self::SESSION_LIST_LIMIT_EXCEEDED => "SessionListLimitExceeded",
            Self::INVALID_REQUEST_DATA_MALFORMED => "InvalidRequestDataMalformed",
            Self::INVALID_REQUEST_DATA_MISSING => "InvalidRequestDataMissing",
            Self::REQUEST_LIMIT_EXCEEDED => "RequestLimitExceeded",
            Self::SESSION_LIMIT_EXCEEDED => "SessionLimitExceeded",
            Self::INVALID_REQUEST_VERSION_OUT_OF_DATE => "InvalidRequestVersionOutOfDate",
            Self::SESSION_ENTITLED_TIME_EXCEEDED => "SessionEntitledTimeExceeded",
            Self::AUTH_FAILURE => "AuthFailure",
            Self::INVALID_AUTHENTICATION_MALFORMED => "InvalidAuthenticationMalformed",
            Self::INVALID_AUTHENTICATION_EXPIRED => "InvalidAuthenticationExpired",
            Self::INVALID_AUTHENTICATION_NOT_FOUND => "InvalidAuthenticationNotFound",
            Self::ENTITLEMENT_FAILURE => "EntitlementFailure",
            Self::INVALID_APP_ID_NOT_AVAILABLE => "InvalidAppIdNotAvailable",
            Self::INVALID_APP_ID_NOT_FOUND => "InvalidAppIdNotFound",
            Self::INVALID_SESSION_ID_MALFORMED => "InvalidSessionIdMalformed",
            Self::INVALID_SESSION_ID_NOT_FOUND => "InvalidSessionIdNotFound",
            Self::EULA_UN_ACCEPTED => "EulaUnAccepted",
            Self::MAINTENANCE_STATUS => "MaintenanceStatus",
            Self::SERVICE_UN_AVAILABLE => "ServiceUnAvailable",
            Self::STEAM_GUARD_REQUIRED => "SteamGuardRequired",
            Self::STEAM_LOGIN_REQUIRED => "SteamLoginRequired",
            Self::STEAM_GUARD_INVALID => "SteamGuardInvalid",
            Self::STEAM_PROFILE_PRIVATE => "SteamProfilePrivate",
            Self::INVALID_COUNTRY_CODE => "InvalidCountryCode",
            Self::INVALID_LANGUAGE_CODE => "InvalidLanguageCode",
            Self::MISSING_COUNTRY_CODE => "MissingCountryCode",
            Self::MISSING_LANGUAGE_CODE => "MissingLanguageCode",
            Self::SESSION_NOT_PAUSED => "SessionNotPaused",
            Self::EMAIL_NOT_VERIFIED => "EmailNotVerified",
            Self::INVALID_AUTHENTICATION_UNSUPPORTED_PROTOCOL => "InvalidAuthenticationUnsupportedProtocol",
            Self::INVALID_AUTHENTICATION_UNKNOWN_TOKEN => "InvalidAuthenticationUnknownToken",
            Self::INVALID_AUTHENTICATION_CREDENTIALS => "InvalidAuthenticationCredentials",
            Self::SESSION_NOT_PLAYING => "SessionNotPlaying",
            Self::INVALID_SERVICE_RESPONSE => "InvalidServiceResponse",
            Self::APP_PATCHING => "AppPatching",
            Self::GAME_NOT_FOUND => "GameNotFound",
            Self::NOT_ENOUGH_CREDITS => "NotEnoughCredits",
            Self::INVITATION_ONLY_REGISTRATION => "InvitationOnlyRegistration",
            Self::REGION_NOT_SUPPORTED_FOR_REGISTRATION => "RegionNotSupportedForRegistration",
            Self::SESSION_TERMINATED_BY_ANOTHER_CLIENT => "SessionTerminatedByAnotherClient",
            Self::DEVICE_ID_ALREADY_USED => "DeviceIdAlreadyUsed",
            Self::SERVICE_NOT_EXIST => "ServiceNotExist",
            Self::SESSION_EXPIRED => "SessionExpired",
            Self::SESSION_LIMIT_PER_DEVICE_REACHED => "SessionLimitPerDeviceReached",
            Self::FORWARDING_ZONE_OUT_OF_CAPACITY => "ForwardingZoneOutOfCapacity",
            Self::REGION_NOT_SUPPORTED_INDEFINITELY => "RegionNotSupportedIndefinitely",
            Self::REGION_BANNED => "RegionBanned",
            Self::REGION_ON_HOLD_FOR_FREE => "RegionOnHoldForFree",
            Self::REGION_ON_HOLD_FOR_PAID => "RegionOnHoldForPaid",
            Self::APP_MAINTENANCE_STATUS => "AppMaintenanceStatus",
            Self::RESOURCE_POOL_NOT_CONFIGURED => "ResourcePoolNotConfigured",
            Self::INSUFFICIENT_VM_CAPACITY => "InsufficientVmCapacity",
            Self::INSUFFICIENT_ROUTE_CAPACITY => "InsufficientRouteCapacity",
            Self::INSUFFICIENT_SCRATCH_SPACE_CAPACITY => "InsufficientScratchSpaceCapacity",
            Self::REQUIRED_SEAT_INSTANCE_TYPE_NOT_SUPPORTED => "RequiredSeatInstanceTypeNotSupported",
            Self::SERVER_SESSION_QUEUE_LENGTH_EXCEEDED => "ServerSessionQueueLengthExceeded",
            Self::REGION_NOT_SUPPORTED_FOR_STREAMING => "RegionNotSupportedForStreaming",
            Self::SESSION_FORWARD_REQUEST_ALLOCATION_TIME_EXPIRED => "SessionForwardRequestAllocationTimeExpired",
            Self::SESSION_FORWARD_GAME_BINARIES_NOT_AVAILABLE => "SessionForwardGameBinariesNotAvailable",
            Self::GAME_BINARIES_NOT_AVAILABLE_IN_REGION => "GameBinariesNotAvailableInRegion",
            Self::UEK_RETRIEVAL_FAILED => "UekRetrievalFailed",
            Self::ENTITLEMENT_FAILURE_FOR_RESOURCE => "EntitlementFailureForResource",
            Self::SESSION_IN_QUEUE_ABANDONED => "SessionInQueueAbandoned",
            Self::MEMBER_TERMINATED => "MemberTerminated",
            Self::SESSION_REMOVED_FROM_QUEUE_MAINTENANCE => "SessionRemovedFromQueueMaintenance",
            Self::ZONE_MAINTENANCE_STATUS => "ZoneMaintenanceStatus",
            Self::GUEST_MODE_CAMPAIGN_DISABLED => "GuestModeCampaignDisabled",
            Self::REGION_NOT_SUPPORTED_ANONYMOUS_ACCESS => "RegionNotSupportedAnonymousAccess",
            Self::INSTANCE_TYPE_NOT_SUPPORTED_IN_SINGLE_REGION => "InstanceTypeNotSupportedInSingleRegion",
            Self::INVALID_ZONE_FOR_QUEUED_SESSION => "InvalidZoneForQueuedSession",
            Self::SESSION_WAITING_ADS_TIME_EXPIRED => "SessionWaitingAdsTimeExpired",
            Self::USER_CANCELLED_WATCHING_ADS => "UserCancelledWatchingAds",
            Self::STREAMING_NOT_ALLOWED_IN_LIMITED_MODE => "StreamingNotAllowedInLimitedMode",
            Self::FORWARD_REQUEST_J_P_M_FAILED => "ForwardRequestJPMFailed",
            Self::MAX_SESSION_NUMBER_LIMIT_EXCEEDED => "MaxSessionNumberLimitExceeded",
            Self::GUEST_MODE_PARTNER_CAPACITY_DISABLED => "GuestModePartnerCapacityDisabled",
            Self::SESSION_REJECTED_NO_CAPACITY => "SessionRejectedNoCapacity",
            Self::SESSION_INSUFFICIENT_PLAYABILITY_LEVEL => "SessionInsufficientPlayabilityLevel",
            Self::FORWARD_REQUEST_L_O_F_N_FAILED => "ForwardRequestLOFNFailed",
            Self::INVALID_TRANSPORT_REQUEST => "InvalidTransportRequest",
            Self::USER_STORAGE_NOT_AVAILABLE => "UserStorageNotAvailable",
            Self::GFN_STORAGE_NOT_AVAILABLE => "GfnStorageNotAvailable",
            Self::APP_NOT_ALLOWED_TO_STREAM => "AppNotAllowedToStream",
            Self::SESSION_SERVER_ERROR_END => "SessionServerErrorEnd",
            Self::SOCKET_ERROR => "SocketError",
            Self::ADDRESS_RESOLVE_FAILED => "AddressResolveFailed",
            Self::CONNECT_FAILED => "ConnectFailed",
            Self::SSL_ERROR => "SslError",
            Self::CONNECTION_TIMEOUT => "ConnectionTimeout",
            Self::DATA_RECEIVE_TIMEOUT => "DataReceiveTimeout",
            Self::PEER_NO_RESPONSE => "PeerNoResponse",
            Self::UNEXPECTED_HTTP_REDIRECT => "UnexpectedHttpRedirect",
            Self::DATA_SEND_FAILURE => "DataSendFailure",
            Self::DATA_RECEIVE_FAILURE => "DataReceiveFailure",
            Self::CERTIFICATE_REJECTED => "CertificateRejected",
            Self::DATA_NOT_ALLOWED => "DataNotAllowed",
            Self::NETWORK_ERROR_UNKNOWN => "NetworkErrorUnknown",
            _ => return None,
        })
    }

    // (title, body) ftl keys for this code, spelled out so a missing translation still
    // shows something readable instead of a raw number
    pub fn message_keys(self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Self::SESSION_SETUP_CANCELLED => (
                "error-gfn-session-setup-cancelled-title",
                "error-gfn-session-setup-cancelled-body",
            ),
            Self::SESSION_SETUP_CANCELLED_DURING_QUEUING => (
                "error-gfn-session-setup-cancelled-during-queuing-title",
                "error-gfn-session-setup-cancelled-during-queuing-body",
            ),
            Self::REQUEST_CANCELLED => (
                "error-gfn-request-cancelled-title",
                "error-gfn-request-cancelled-body",
            ),
            Self::SYSTEM_SLEEP_DURING_SESSION_SETUP => (
                "error-gfn-system-sleep-during-session-setup-title",
                "error-gfn-system-sleep-during-session-setup-body",
            ),
            Self::NO_INTERNET_DURING_SESSION_SETUP => (
                "error-gfn-no-internet-during-session-setup-title",
                "error-gfn-no-internet-during-session-setup-body",
            ),
            Self::INVALID_OPERATION => (
                "error-gfn-invalid-operation-title",
                "error-gfn-invalid-operation-body",
            ),
            Self::NETWORK_ERROR => (
                "error-gfn-network-error-title",
                "error-gfn-network-error-body",
            ),
            Self::AUTH_TOKEN_NOT_UPDATED => (
                "error-gfn-auth-token-not-updated-title",
                "error-gfn-auth-token-not-updated-body",
            ),
            Self::RESPONSE_PARSE_FAILURE => (
                "error-gfn-response-parse-failure-title",
                "error-gfn-response-parse-failure-body",
            ),
            Self::INVALID_SERVER_RESPONSE => (
                "error-gfn-invalid-server-response-title",
                "error-gfn-invalid-server-response-body",
            ),
            Self::D_O_M_EXCEPTION_IN_SESSION_CONTROL => (
                "error-gfn-dom-exception-in-session-control-title",
                "error-gfn-dom-exception-in-session-control-body",
            ),
            Self::AUTH_TOKEN_UPDATE_TIMEOUT => (
                "error-gfn-auth-token-update-timeout-title",
                "error-gfn-auth-token-update-timeout-body",
            ),
            Self::REQUEST_FORBIDDEN => (
                "error-gfn-request-forbidden-title",
                "error-gfn-request-forbidden-body",
            ),
            Self::SERVER_INTERNAL_TIMEOUT => (
                "error-gfn-server-internal-timeout-title",
                "error-gfn-server-internal-timeout-body",
            ),
            Self::SERVER_INTERNAL_ERROR => (
                "error-gfn-server-internal-error-title",
                "error-gfn-server-internal-error-body",
            ),
            Self::SERVER_INVALID_REQUEST => (
                "error-gfn-server-invalid-request-title",
                "error-gfn-server-invalid-request-body",
            ),
            Self::SESSION_LIST_LIMIT_EXCEEDED => (
                "error-gfn-session-list-limit-exceeded-title",
                "error-gfn-session-list-limit-exceeded-body",
            ),
            Self::SESSION_LIMIT_EXCEEDED => (
                "error-gfn-session-limit-exceeded-title",
                "error-gfn-session-limit-exceeded-body",
            ),
            Self::SESSION_ENTITLED_TIME_EXCEEDED => (
                "error-gfn-session-entitled-time-exceeded-title",
                "error-gfn-session-entitled-time-exceeded-body",
            ),
            Self::AUTH_FAILURE => (
                "error-gfn-auth-failure-title",
                "error-gfn-auth-failure-body",
            ),
            Self::INVALID_AUTHENTICATION_EXPIRED => (
                "error-gfn-invalid-authentication-expired-title",
                "error-gfn-invalid-authentication-expired-body",
            ),
            Self::ENTITLEMENT_FAILURE => (
                "error-gfn-entitlement-failure-title",
                "error-gfn-entitlement-failure-body",
            ),
            Self::INVALID_APP_ID_NOT_AVAILABLE => (
                "error-gfn-invalid-app-id-not-available-title",
                "error-gfn-invalid-app-id-not-available-body",
            ),
            Self::INVALID_APP_ID_NOT_FOUND => (
                "error-gfn-invalid-app-id-not-found-title",
                "error-gfn-invalid-app-id-not-found-body",
            ),
            Self::EULA_UN_ACCEPTED => (
                "error-gfn-eula-un-accepted-title",
                "error-gfn-eula-un-accepted-body",
            ),
            Self::MAINTENANCE_STATUS => (
                "error-gfn-maintenance-status-title",
                "error-gfn-maintenance-status-body",
            ),
            Self::SERVICE_UN_AVAILABLE => (
                "error-gfn-service-un-available-title",
                "error-gfn-service-un-available-body",
            ),
            Self::STEAM_GUARD_REQUIRED => (
                "error-gfn-steam-guard-required-title",
                "error-gfn-steam-guard-required-body",
            ),
            Self::STEAM_LOGIN_REQUIRED => (
                "error-gfn-steam-login-required-title",
                "error-gfn-steam-login-required-body",
            ),
            Self::STEAM_GUARD_INVALID => (
                "error-gfn-steam-guard-invalid-title",
                "error-gfn-steam-guard-invalid-body",
            ),
            Self::STEAM_PROFILE_PRIVATE => (
                "error-gfn-steam-profile-private-title",
                "error-gfn-steam-profile-private-body",
            ),
            Self::EMAIL_NOT_VERIFIED => (
                "error-gfn-email-not-verified-title",
                "error-gfn-email-not-verified-body",
            ),
            Self::APP_PATCHING => (
                "error-gfn-app-patching-title",
                "error-gfn-app-patching-body",
            ),
            Self::GAME_NOT_FOUND => (
                "error-gfn-game-not-found-title",
                "error-gfn-game-not-found-body",
            ),
            Self::NOT_ENOUGH_CREDITS => (
                "error-gfn-not-enough-credits-title",
                "error-gfn-not-enough-credits-body",
            ),
            Self::SESSION_TERMINATED_BY_ANOTHER_CLIENT => (
                "error-gfn-session-terminated-by-another-client-title",
                "error-gfn-session-terminated-by-another-client-body",
            ),
            Self::SESSION_EXPIRED => (
                "error-gfn-session-expired-title",
                "error-gfn-session-expired-body",
            ),
            Self::SESSION_LIMIT_PER_DEVICE_REACHED => (
                "error-gfn-session-limit-per-device-reached-title",
                "error-gfn-session-limit-per-device-reached-body",
            ),
            Self::FORWARDING_ZONE_OUT_OF_CAPACITY => (
                "error-gfn-forwarding-zone-out-of-capacity-title",
                "error-gfn-forwarding-zone-out-of-capacity-body",
            ),
            Self::REGION_NOT_SUPPORTED_INDEFINITELY => (
                "error-gfn-region-not-supported-indefinitely-title",
                "error-gfn-region-not-supported-indefinitely-body",
            ),
            Self::REGION_BANNED => (
                "error-gfn-region-banned-title",
                "error-gfn-region-banned-body",
            ),
            Self::REGION_ON_HOLD_FOR_FREE => (
                "error-gfn-region-on-hold-for-free-title",
                "error-gfn-region-on-hold-for-free-body",
            ),
            Self::REGION_ON_HOLD_FOR_PAID => (
                "error-gfn-region-on-hold-for-paid-title",
                "error-gfn-region-on-hold-for-paid-body",
            ),
            Self::APP_MAINTENANCE_STATUS => (
                "error-gfn-app-maintenance-status-title",
                "error-gfn-app-maintenance-status-body",
            ),
            Self::INSUFFICIENT_VM_CAPACITY => (
                "error-gfn-insufficient-vm-capacity-title",
                "error-gfn-insufficient-vm-capacity-body",
            ),
            Self::SERVER_SESSION_QUEUE_LENGTH_EXCEEDED => (
                "error-gfn-server-session-queue-length-exceeded-title",
                "error-gfn-server-session-queue-length-exceeded-body",
            ),
            Self::REGION_NOT_SUPPORTED_FOR_STREAMING => (
                "error-gfn-region-not-supported-for-streaming-title",
                "error-gfn-region-not-supported-for-streaming-body",
            ),
            Self::GAME_BINARIES_NOT_AVAILABLE_IN_REGION => (
                "error-gfn-game-binaries-not-available-in-region-title",
                "error-gfn-game-binaries-not-available-in-region-body",
            ),
            Self::SESSION_IN_QUEUE_ABANDONED => (
                "error-gfn-session-in-queue-abandoned-title",
                "error-gfn-session-in-queue-abandoned-body",
            ),
            Self::MEMBER_TERMINATED => (
                "error-gfn-member-terminated-title",
                "error-gfn-member-terminated-body",
            ),
            Self::SESSION_REMOVED_FROM_QUEUE_MAINTENANCE => (
                "error-gfn-session-removed-from-queue-maintenance-title",
                "error-gfn-session-removed-from-queue-maintenance-body",
            ),
            Self::ZONE_MAINTENANCE_STATUS => (
                "error-gfn-zone-maintenance-status-title",
                "error-gfn-zone-maintenance-status-body",
            ),
            Self::SESSION_WAITING_ADS_TIME_EXPIRED => (
                "error-gfn-session-waiting-ads-time-expired-title",
                "error-gfn-session-waiting-ads-time-expired-body",
            ),
            Self::USER_CANCELLED_WATCHING_ADS => (
                "error-gfn-user-cancelled-watching-ads-title",
                "error-gfn-user-cancelled-watching-ads-body",
            ),
            Self::STREAMING_NOT_ALLOWED_IN_LIMITED_MODE => (
                "error-gfn-streaming-not-allowed-in-limited-mode-title",
                "error-gfn-streaming-not-allowed-in-limited-mode-body",
            ),
            Self::MAX_SESSION_NUMBER_LIMIT_EXCEEDED => (
                "error-gfn-max-session-number-limit-exceeded-title",
                "error-gfn-max-session-number-limit-exceeded-body",
            ),
            Self::SESSION_REJECTED_NO_CAPACITY => (
                "error-gfn-session-rejected-no-capacity-title",
                "error-gfn-session-rejected-no-capacity-body",
            ),
            Self::SESSION_INSUFFICIENT_PLAYABILITY_LEVEL => (
                "error-gfn-session-insufficient-playability-level-title",
                "error-gfn-session-insufficient-playability-level-body",
            ),
            Self::USER_STORAGE_NOT_AVAILABLE => (
                "error-gfn-user-storage-not-available-title",
                "error-gfn-user-storage-not-available-body",
            ),
            Self::GFN_STORAGE_NOT_AVAILABLE => (
                "error-gfn-gfn-storage-not-available-title",
                "error-gfn-gfn-storage-not-available-body",
            ),
            Self::APP_NOT_ALLOWED_TO_STREAM => (
                "error-gfn-app-not-allowed-to-stream-title",
                "error-gfn-app-not-allowed-to-stream-body",
            ),
            Self::SOCKET_ERROR => (
                "error-gfn-socket-error-title",
                "error-gfn-socket-error-body",
            ),
            Self::ADDRESS_RESOLVE_FAILED => (
                "error-gfn-address-resolve-failed-title",
                "error-gfn-address-resolve-failed-body",
            ),
            Self::CONNECT_FAILED => (
                "error-gfn-connect-failed-title",
                "error-gfn-connect-failed-body",
            ),
            Self::SSL_ERROR => (
                "error-gfn-ssl-error-title",
                "error-gfn-ssl-error-body",
            ),
            Self::CONNECTION_TIMEOUT => (
                "error-gfn-connection-timeout-title",
                "error-gfn-connection-timeout-body",
            ),
            Self::DATA_RECEIVE_TIMEOUT => (
                "error-gfn-data-receive-timeout-title",
                "error-gfn-data-receive-timeout-body",
            ),
            Self::PEER_NO_RESPONSE => (
                "error-gfn-peer-no-response-title",
                "error-gfn-peer-no-response-body",
            ),
            Self::CERTIFICATE_REJECTED => (
                "error-gfn-certificate-rejected-title",
                "error-gfn-certificate-rejected-body",
            ),
            _ => return None,
        })
    }

    // account already has a session open somewhere, 3 diff codes for the same thing basically
    pub fn is_session_conflict(self) -> bool {
        matches!(
            self,
            Self::SESSION_LIMIT_EXCEEDED
                | Self::SESSION_LIMIT_PER_DEVICE_REACHED
                | Self::MAX_SESSION_NUMBER_LIMIT_EXCEEDED
        )
    }

    // temporary/busy stuff, safe to just retry
    pub fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::NETWORK_ERROR
                | Self::SERVER_INTERNAL_TIMEOUT
                | Self::SERVER_INTERNAL_ERROR
                | Self::FORWARDING_ZONE_OUT_OF_CAPACITY
                | Self::INSUFFICIENT_VM_CAPACITY
                | Self::SESSION_REJECTED_NO_CAPACITY
                | Self::CONNECTION_TIMEOUT
                | Self::DATA_RECEIVE_TIMEOUT
                | Self::PEER_NO_RESPONSE
        )
    }

    // token is dead, gotta make them log in again
    pub fn needs_reauth(self) -> bool {
        matches!(
            self,
            Self::AUTH_TOKEN_NOT_UPDATED
                | Self::AUTH_TOKEN_UPDATE_TIMEOUT
                | Self::AUTH_FAILURE
                | Self::INVALID_AUTHENTICATION_MALFORMED
                | Self::INVALID_AUTHENTICATION_EXPIRED
                | Self::INVALID_AUTHENTICATION_NOT_FOUND
                | Self::INVALID_AUTHENTICATION_UNSUPPORTED_PROTOCOL
                | Self::INVALID_AUTHENTICATION_UNKNOWN_TOKEN
                | Self::INVALID_AUTHENTICATION_CREDENTIALS
        )
    }
}

// carries the real code alongside the text, since anyhow eats everything but the message
// error screen does downcast_ref::<GfnError>() to grab this back out
#[derive(Debug, Clone)]
pub struct GfnError {
    pub code: GfnErrorCode,
    // http status if it was an http failure
    pub http_status: Option<u16>,
    // nvidia's raw wording, used when the code has no message of its own
    pub status_description: Option<String>,
    // op + raw body, for the logs
    pub detail: String,
}

impl GfnError {
    pub fn new(code: GfnErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            http_status: None,
            status_description: None,
            detail: detail.into(),
        }
    }

    pub fn with_http_status(mut self, status: u16) -> Self {
        self.http_status = Some(status);
        self
    }

    pub fn with_description(mut self, description: Option<String>) -> Self {
        self.status_description = description;
        self
    }

    // short id for the fallback msg: description if we got one, else name, else just the number
    pub fn summary(&self) -> String {
        if let Some(description) = self.status_description.as_deref().filter(|d| !d.is_empty()) {
            return format!("{description} ({})", self.code.0);
        }
        match self.code.name() {
            Some(name) => format!("{name} ({})", self.code.0),
            None => self.code.0.to_string(),
        }
    }
}

impl std::fmt::Display for GfnError {
    // same shape logs already had, just tacking the code on the end now
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} [gfn {}]", self.detail, self.summary())
    }
}

impl std::error::Error for GfnError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_success_sentinel_is_not_a_server_error() {
        assert_eq!(GfnErrorCode::from_cloudmatch(1, None), GfnErrorCode::SUCCESS);
    }

    #[test]
    fn server_codes_are_the_base_plus_the_status_code() {
        assert_eq!(
            GfnErrorCode::from_cloudmatch(11, None),
            GfnErrorCode::SESSION_LIMIT_EXCEEDED
        );
        assert_eq!(
            GfnErrorCode::from_cloudmatch(41, None),
            GfnErrorCode::APP_PATCHING
        );
        assert_eq!(
            GfnErrorCode::from_cloudmatch(50, None),
            GfnErrorCode::SESSION_LIMIT_PER_DEVICE_REACHED
        );
    }

    // unifiedErrorCode shouldnt win when we already have a specific code
    #[test]
    fn the_unified_code_only_replaces_a_useless_one() {
        // Generic: the unified code is all the detail there is.
        assert_eq!(
            GfnErrorCode::from_cloudmatch(4, Some(3237101584)),
            GfnErrorCode::CONNECTION_TIMEOUT
        );
        // Specific: keep it.
        assert_eq!(
            GfnErrorCode::from_cloudmatch(11, Some(3237101584)),
            GfnErrorCode::SESSION_LIMIT_EXCEEDED
        );
    }

    // negative/huge unifiedErrorCode shouldnt panic or wrap around into a real code
    #[test]
    fn an_out_of_range_unified_code_is_ignored() {
        assert_eq!(
            GfnErrorCode::from_cloudmatch(4, Some(-1)),
            GfnErrorCode::SERVER_INTERNAL_ERROR
        );
    }

    #[test]
    fn every_session_conflict_code_is_recognised() {
        for code in [
            GfnErrorCode::SESSION_LIMIT_EXCEEDED,
            GfnErrorCode::SESSION_LIMIT_PER_DEVICE_REACHED,
            GfnErrorCode::MAX_SESSION_NUMBER_LIMIT_EXCEEDED,
        ] {
            assert!(code.is_session_conflict(), "{:?}", code.name());
        }
        assert!(!GfnErrorCode::APP_PATCHING.is_session_conflict());
    }

    #[test]
    fn expired_and_malformed_logins_both_need_reauth() {
        assert!(GfnErrorCode::INVALID_AUTHENTICATION_EXPIRED.needs_reauth());
        assert!(GfnErrorCode::AUTH_FAILURE.needs_reauth());
        // Capacity problems must not send the player back to the login screen.
        assert!(!GfnErrorCode::INSUFFICIENT_VM_CAPACITY.needs_reauth());
    }

    #[test]
    fn capacity_is_retryable_but_a_refusal_is_not() {
        assert!(GfnErrorCode::INSUFFICIENT_VM_CAPACITY.is_retryable());
        assert!(GfnErrorCode::SERVER_INTERNAL_ERROR.is_retryable());
        assert!(!GfnErrorCode::REGION_BANNED.is_retryable());
        assert!(!GfnErrorCode::SESSION_LIMIT_EXCEEDED.is_retryable());
    }

    #[test]
    fn a_description_fallback_reuses_an_existing_codes_wording() {
        assert_eq!(
            GfnErrorCode::from_description("SESSION_LIMIT_PER_DEVICE_EXCEEDED_STATUS"),
            Some(GfnErrorCode::SESSION_LIMIT_EXCEEDED)
        );
        assert_eq!(GfnErrorCode::from_description("something new"), None);
    }

    // specific patterns gotta be checked before the broad ones or this fails
    #[test]
    fn a_specific_description_beats_the_generic_auth_catch_all() {
        assert_eq!(
            GfnErrorCode::from_description("ENTITLEMENT_AUTH_FAILURE"),
            Some(GfnErrorCode::ENTITLEMENT_FAILURE)
        );
    }

    #[test]
    fn an_unknown_code_survives_instead_of_being_lost() {
        let unknown = GfnErrorCode(4242);
        assert_eq!(unknown.name(), None);
        assert_eq!(unknown.message_keys(), None);
        assert_eq!(unknown.0, 4242);
    }

    // checks every key we reference actually exists in both ftl files, otherwise the
    // player just sees the raw key on screen lol
    #[test]
    fn every_message_key_exists_in_both_locales() {
        const EN: &str = include_str!("../i18n/en-US.ftl");
        const ES: &str = include_str!("../i18n/es-ES.ftl");

        fn defines(source: &str, key: &str) -> bool {
            source
                .lines()
                .any(|line| line.split('=').next().is_some_and(|id| id.trim() == key))
        }

        for code in GfnErrorCode::ALL {
            let Some((title, body)) = code.message_keys() else {
                continue;
            };
            for key in [title, body] {
                assert!(defines(EN, key), "en-US.ftl is missing {key}");
                assert!(defines(ES, key), "es-ES.ftl is missing {key}");
            }
        }

        for key in ["error-gfn-unknown-title", "error-gfn-unknown-body"] {
            assert!(defines(EN, key), "en-US.ftl is missing {key}");
            assert!(defines(ES, key), "es-ES.ftl is missing {key}");
        }
    }

    #[test]
    fn message_keys_are_unique_per_code() {
        let mut seen = std::collections::HashSet::new();
        for code in GfnErrorCode::ALL {
            if let Some((title, _)) = code.message_keys() {
                assert!(seen.insert(title), "{title} is claimed by two codes");
            }
        }
    }

    #[test]
    fn message_keys_come_in_pairs() {
        for code in [
            GfnErrorCode::SESSION_LIMIT_EXCEEDED,
            GfnErrorCode::APP_PATCHING,
            GfnErrorCode::AUTH_FAILURE,
            GfnErrorCode::REGION_NOT_SUPPORTED_FOR_STREAMING,
        ] {
            let (title, body) = code.message_keys().expect("should have wording");
            assert!(title.ends_with("-title"), "{title}");
            assert!(body.ends_with("-body"), "{body}");
            assert_eq!(title.trim_end_matches("-title"), body.trim_end_matches("-body"));
        }
    }
}
