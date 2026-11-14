//! 応答を返した後に一度だけ処理を起動する本体ラッパー。
//!
//! 「応答を返した後」とは、handler が応答を確定して返し、HTTP サーバーが応答本体を最後まで取り出した
//! (`poll_frame` が `Poll::Ready(None)` を返した) か、本体を破棄した (`Drop`) 後を指す。**ソケットへの
//! 書き込み完了やクライアントの受信完了との前後は保証しない**: hyper は本体を取り出し切ってから
//! ソケットへ flush するので、本体の層からは書き込み完了を知る手段が無い。保証するのは、応答の確定前や
//! 本体を取り出している最中に処理を始めない (= 遅い外部 I/O が応答を遅らせず、ハンドラ内のトランザクション
//! でも外部 I/O をしない) ことだけである。
//!
//! 最後のデータフレームを取り出しても、その時点では起動しない (`is_end_stream` が真でも `Ready(None)` を
//! 返すまでは起動しない)。本体が `Ready(None)` まで取り出されずに破棄された場合は `Drop` で起動する。

use axum::body::{Body, Bytes};
use axum::response::Response;
use http_body::{Frame, SizeHint};
use std::pin::Pin;
use std::task::{Context, Poll};

/// 本体を取り出し終えた後 (または破棄された後) に一度だけ呼ぶ処理。
pub(crate) type StartFn = Box<dyn FnOnce() + Send + 'static>;

/// `start` を、`response` の本体を取り出し終えた後 (または本体の破棄後) に一度だけ呼ぶ形で結び付ける。
pub(crate) fn start_after_body(
    response: Response,
    start: impl FnOnce() + Send + 'static,
) -> Response {
    response.map(|body| {
        Body::new(StartAfterBody {
            inner: body,
            start: Some(Box::new(start)),
        })
    })
}

/// `has_work` が偽なら本体を包まずそのまま返し、真なら [`start_after_body`] で結び付ける。
pub(crate) fn start_after_body_if(
    response: Response,
    has_work: bool,
    start: impl FnOnce() + Send + 'static,
) -> Response {
    if has_work {
        start_after_body(response, start)
    } else {
        response
    }
}

/// 本体を素通しし、`Poll::Ready(None)` を返した時、またはそれより前に破棄された時に一度だけ `start` を呼ぶ。
struct StartAfterBody {
    inner: Body,
    start: Option<StartFn>,
}

impl StartAfterBody {
    fn start_once(&mut self) {
        if let Some(start) = self.start.take() {
            start();
        }
    }
}

impl http_body::Body for StartAfterBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.inner).poll_frame(cx);
        if matches!(poll, Poll::Ready(None)) {
            this.start_once();
        }
        poll
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for StartAfterBody {
    fn drop(&mut self) {
        self.start_once();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use std::collections::VecDeque;
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// 複数フレームの本体。hyper が見る `is_end_stream` は最後のデータフレームを返した直後に真になる。
    struct Frames(VecDeque<&'static str>);

    impl http_body::Body for Frames {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
            Poll::Ready(
                self.0
                    .pop_front()
                    .map(|text| Ok(Frame::data(Bytes::from(text)))),
            )
        }

        fn is_end_stream(&self) -> bool {
            self.0.is_empty()
        }

        fn size_hint(&self) -> SizeHint {
            SizeHint::with_exact(self.0.iter().map(|text| text.len() as u64).sum())
        }
    }

    fn wrapped(frames: &[&'static str]) -> (Body, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let response = Response::new(Body::new(Frames(frames.iter().copied().collect())));
        let response = start_after_body(response, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        (response.into_body(), calls)
    }

    #[tokio::test]
    async fn it_starts_only_after_the_body_has_returned_none() {
        let (mut body, calls) = wrapped(&["a", "b"]);
        assert!(body.frame().await.unwrap().unwrap().is_data());
        assert_eq!(calls.load(Ordering::SeqCst), 0, "after the first frame");
        assert!(body.frame().await.unwrap().unwrap().is_data());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "the last data frame alone does not start it, even though the body is at its end"
        );
        assert!(http_body::Body::is_end_stream(&body));
        assert!(body.frame().await.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1, "after None");
        drop(body);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "drop after None is not a second start"
        );
    }

    #[tokio::test]
    async fn a_single_frame_body_starts_when_it_is_dropped_without_reading_none() {
        let (mut body, calls) = wrapped(&["only"]);
        assert!(body.frame().await.unwrap().unwrap().is_data());
        assert_eq!(calls.load(Ordering::SeqCst), 0, "after the only frame");
        drop(body);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "after drop");
    }

    #[tokio::test]
    async fn a_body_dropped_without_being_polled_starts_once() {
        let (body, calls) = wrapped(&["a", "b"]);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        drop(body);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn nothing_is_wrapped_or_started_when_there_is_no_work() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let response = Response::new(Body::from("plain"));
        let response = start_after_body_if(response, false, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"plain");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn the_wrapper_keeps_the_exact_size_hint() {
        let (body, _) = wrapped(&["abc", "de"]);
        assert_eq!(http_body::Body::size_hint(&body).exact(), Some(5));
    }
}
