//! AWS Signature Version 4 signing of S3 requests.
//!
//! Every request to the bucket carries a signature over its payload hash: S3 requires the hash
//! to be part of the signature, must not have its URI path normalized and only single-encodes
//! URI paths. The [request](super::request) layer applies the headers returned here.

use std::time::SystemTime;

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{
    PayloadChecksumKind, PercentEncodingMode, SignableBody, SignableRequest, SigningSettings,
    UriPathNormalizationMode, sign,
};
use aws_sigv4::sign::v4;
use aws_smithy_runtime_api::client::identity::Identity;
use url::Url;

use super::{Error, S3};

/// The name used to identify our credentials provider.
const CREDENTIALS_PROVIDER_NAME: &str = "zeta-mirror";

impl S3 {
    /// Signs a request for the given URL and body, returning the headers to apply to it.
    pub(super) fn signing_headers(
        &self,
        method: &str,
        url: &Url,
        body: SignableBody<'_>,
    ) -> Result<Vec<(&'static str, String)>, Error> {
        let credentials = Credentials::new(
            self.access_key_id.as_str(),
            self.secret_access_key.as_str(),
            None,
            None,
            CREDENTIALS_PROVIDER_NAME,
        );
        let identity: Identity = credentials.into();
        let mut settings = SigningSettings::default();
        // S3 requires the payload hash to be part of the signature, must not have its URI path
        // normalized, and only single-encodes URI paths.
        settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
        settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
        settings.percent_encoding_mode = PercentEncodingMode::Single;
        let params: aws_sigv4::http_request::SigningParams<'_> = v4::SigningParams::builder()
            .identity(&identity)
            .region(&self.region)
            .name("s3")
            .time(SystemTime::now())
            .settings(settings)
            .build()?
            .into();

        let request = SignableRequest::new(method, url.as_str(), std::iter::empty(), body)?;
        let (instructions, _signature) = sign(request, &params)?.into_parts();
        let (headers, _query) = instructions.into_parts();

        Ok(headers
            .into_iter()
            .map(|header| (header.name(), header.value().to_string()))
            .collect())
    }
}

/// Applies the signed `headers` to `request`.
pub(super) fn apply_headers(
    mut request: reqwest::RequestBuilder,
    headers: &[(&'static str, String)],
) -> reqwest::RequestBuilder {
    for (name, value) in headers {
        request = request.header(*name, value.as_str());
    }

    request
}
