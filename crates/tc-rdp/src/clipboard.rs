//! Text clipboard over CLIPRDR: the backend only reports what the server
//! did; the session loop answers through the channel (it owns it).

use ironrdp_cliprdr::backend::CliprdrBackend;
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardGeneralCapabilityFlags, FileContentsRequest,
    FileContentsResponse, FormatDataRequest, FormatDataResponse, LockDataId,
};

/// What the server asked or sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClipEvent {
    /// The channel is up: announce our clipboard.
    Ready,
    /// The remote clipboard changed and holds text.
    RemoteText,
    /// The remote text arrived.
    Text(String),
    /// The server wants our text.
    DataRequest,
}

#[derive(Debug)]
pub(crate) struct TextClipboard {
    pub(crate) events: tokio::sync::mpsc::UnboundedSender<ClipEvent>,
}

ironrdp_core::impl_as_any!(TextClipboard);

impl CliprdrBackend for TextClipboard {
    fn temporary_directory(&self) -> &str {
        ""
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::empty()
    }

    fn on_ready(&mut self) {}

    fn on_request_format_list(&mut self) {
        let _ = self.events.send(ClipEvent::Ready);
    }

    fn on_process_negotiated_capabilities(&mut self, _: ClipboardGeneralCapabilityFlags) {}

    fn on_remote_copy(&mut self, formats: &[ClipboardFormat]) {
        if formats
            .iter()
            .any(|f| f.id == ClipboardFormatId::CF_UNICODETEXT)
        {
            let _ = self.events.send(ClipEvent::RemoteText);
        }
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        if request.format == ClipboardFormatId::CF_UNICODETEXT {
            let _ = self.events.send(ClipEvent::DataRequest);
        }
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        if response.is_error() {
            return;
        }
        if let Ok(text) = response.to_unicode_string() {
            let text = text.trim_end_matches('\0').to_string();
            let _ = self.events.send(ClipEvent::Text(text));
        }
    }

    fn on_file_contents_request(&mut self, _: FileContentsRequest) {}

    fn on_file_contents_response(&mut self, _: FileContentsResponse<'_>) {}

    fn on_lock(&mut self, _: LockDataId) {}

    fn on_unlock(&mut self, _: LockDataId) {}
}

/// The formats we offer: Unicode text, when there is some.
pub(crate) fn offered(text: Option<&str>) -> Vec<ClipboardFormat> {
    match text {
        Some(_) => vec![ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)],
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironrdp_cliprdr::CliprdrClient;
    use ironrdp_cliprdr::pdu::{ClipboardPdu, FormatList, OwnedFormatDataResponse};
    use ironrdp_svc::SvcProcessor as _;

    fn feed(client: &mut CliprdrClient, pdu: &ClipboardPdu<'_>) {
        let bytes = ironrdp_core::encode_vec(pdu).unwrap();
        client.process(&bytes).unwrap();
    }

    #[test]
    fn server_messages_become_events() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut client = CliprdrClient::new(Box::new(TextClipboard { events: tx }));
        feed(&mut client, &ClipboardPdu::MonitorReady);
        assert_eq!(rx.try_recv().ok(), Some(ClipEvent::Ready));

        let text = offered(Some("x"));
        feed(
            &mut client,
            &ClipboardPdu::FormatList(FormatList::new_unicode(&text, true).unwrap()),
        );
        assert_eq!(rx.try_recv().ok(), Some(ClipEvent::RemoteText));

        feed(
            &mut client,
            &ClipboardPdu::FormatDataRequest(FormatDataRequest {
                format: ClipboardFormatId::CF_UNICODETEXT,
            }),
        );
        assert_eq!(rx.try_recv().ok(), Some(ClipEvent::DataRequest));

        let data = OwnedFormatDataResponse::new_unicode_string("olá");
        feed(&mut client, &ClipboardPdu::FormatDataResponse(data));
        assert_eq!(rx.try_recv().ok(), Some(ClipEvent::Text("olá".into())));

        // Other formats only (an image): nothing to fetch.
        let image = [ClipboardFormat::new(ClipboardFormatId::new(8))];
        feed(
            &mut client,
            &ClipboardPdu::FormatList(FormatList::new_unicode(&image, true).unwrap()),
        );
        assert!(rx.try_recv().is_err());
        assert!(offered(None).is_empty());
    }
}
