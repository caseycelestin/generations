//! Requests from control-side callers to a real-time side, each answered by
//! exactly one reply. One request type per channel, so neither end converts
//! what it receives; a feature with several kinds of request makes them one
//! enum. Knows nothing of audio.
//!
//! - On the real-time side every operation is wait-free and bounded: it
//!   never allocates, frees, locks or blocks. The control side may.
//! - Memory is fixed at creation.
//! - A reservation holds room for a request, its cancel, one status
//!   question at a time and the answers to both. Past it, only a second
//!   question while one is out is refused; nothing the real-time side sends
//!   is lost.
//! - Each direction is in order.
//!
//! Each side's end is in its own file, so `realtime.rs` can be held to the
//! real-time rules alone.

mod control;
mod realtime;

pub use control::{
    ControlEnd, PendingStatus, RequestHandle, RequestPermit, ReserveError, StatusError,
    TryRecvStatusError, TryWaitError,
};
pub use realtime::{RealtimeEnd, Received, ReplyPermit, StatusPermit};

use rtrb::RingBuffer;

/// Pairs a request type with its reply and with what it answers while
/// open, when asked its status.
pub trait Request {
    type Reply;
    type Status;
}

/// Unique within a channel and never reused; names, not ages, so they do
/// not order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(pub(crate) u64);

/// What the ring to the real-time side carries.
#[derive(Debug)]
enum ToRealtime<Q> {
    Request(RequestId, Q),
    Cancel(RequestId),
    Status(RequestId),
}

/// What the ring back carries.
enum FromRealtime<Q: Request> {
    Reply(RequestId, Q::Reply),
    Status(RequestId, Q::Status),
}

/// `capacity` is in requests: the most that can await their reply at once.
/// The ring out holds three times that, for each one's cancel and status
/// question, and the ring back twice, for each one's status answer, so
/// neither ring ever fills.
pub fn channel<Q: Request>(capacity: usize) -> (ControlEnd<Q>, RealtimeEnd<Q>) {
    channel_with_headroom(capacity, 0)
}

/// As `channel`, with the last `headroom` slots of `capacity` kept for
/// `try_reserve_including_headroom`, so ordinary requests cannot crowd it out.
pub fn channel_with_headroom<Q: Request>(
    capacity: usize,
    headroom: usize,
) -> (ControlEnd<Q>, RealtimeEnd<Q>) {
    let (to_realtime, from_control) = RingBuffer::new(3 * capacity);
    let (to_control, from_realtime) = RingBuffer::new(2 * capacity);
    let control = ControlEnd::new(to_realtime, from_realtime, capacity, headroom);
    let realtime = RealtimeEnd::new(from_control, to_control);
    (control, realtime)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// A request carrying a number, answered with numbers.
    #[derive(Debug)]
    struct Ask(u32);

    impl Request for Ask {
        type Reply = u32;
        type Status = u32;
    }

    /// A request whose reply can be watched being freed.
    #[derive(Debug)]
    struct Hold;

    impl Request for Hold {
        type Reply = Arc<()>;
        type Status = ();
    }

    fn open(control: &ControlEnd<Ask>, number: u32) -> RequestHandle<Ask> {
        control.try_reserve().unwrap().open(Ask(number))
    }

    /// Takes the next request off the ring and closes it with its number.
    fn close_next(realtime: &mut RealtimeEnd<Ask>) {
        let Some(Received::Request(permit, Ask(number))) = realtime.try_recv() else {
            panic!("no request was waiting");
        };
        realtime.close(permit, number);
    }

    /// Takes the next request off the ring, keeping its permit.
    fn receive(realtime: &mut RealtimeEnd<Ask>) -> ReplyPermit {
        let Some(Received::Request(permit, _)) = realtime.try_recv() else {
            panic!("no request was waiting");
        };
        permit
    }

    /// Takes the next status question off the ring.
    fn question(realtime: &mut RealtimeEnd<Ask>) -> StatusPermit {
        let Some(Received::Status(permit)) = realtime.try_recv() else {
            panic!("no question was waiting");
        };
        permit
    }

    #[test]
    fn reserving_is_full_once_capacity_requests_are_in_flight() {
        let (control, _realtime) = channel::<Ask>(2);
        let _first = open(&control, 1);
        let _second = open(&control, 2);
        assert_eq!(control.try_reserve().err(), Some(ReserveError::Full));
    }

    #[test]
    fn a_permit_holds_its_room_until_opened() {
        let (control, _realtime) = channel::<Ask>(1);
        let _permit = control.try_reserve().unwrap();
        assert_eq!(control.try_reserve().err(), Some(ReserveError::Full));
    }

    #[test]
    fn a_permit_dropped_unused_gives_its_room_back() {
        let (control, _realtime) = channel::<Ask>(1);
        drop(control.try_reserve().unwrap());
        assert!(control.try_reserve().is_ok());
    }

    #[test]
    fn try_reserve_leaves_the_headroom() {
        let (control, _realtime) = channel_with_headroom::<Ask>(2, 1);
        let _first = open(&control, 1);
        assert_eq!(control.try_reserve().err(), Some(ReserveError::Full));
        assert!(control.try_reserve_including_headroom().is_ok());
    }

    #[test]
    fn the_headroom_is_part_of_the_capacity() {
        let (control, _realtime) = channel_with_headroom::<Ask>(2, 1);
        let _first = open(&control, 1);
        let _second = control
            .try_reserve_including_headroom()
            .unwrap()
            .open(Ask(2));
        assert_eq!(
            control.try_reserve_including_headroom().err(),
            Some(ReserveError::Full)
        );
    }

    #[test]
    fn a_slot_is_held_until_its_reply_is_taken() {
        let (control, mut realtime) = channel::<Ask>(1);
        let mut handle = open(&control, 1);
        close_next(&mut realtime);
        assert_eq!(control.try_reserve().err(), Some(ReserveError::Full));

        assert_eq!(handle.try_wait(), Ok(1));

        assert!(control.try_reserve().is_ok());
    }

    #[test]
    fn a_dropped_handle_gives_its_slot_back_once_its_reply_arrives() {
        let (control, mut realtime) = channel::<Ask>(1);
        drop(open(&control, 1));
        assert_eq!(control.try_reserve().err(), Some(ReserveError::Full));

        close_next(&mut realtime);

        assert!(control.try_reserve().is_ok());
    }

    #[test]
    fn reserving_is_disconnected_once_the_realtime_end_is_gone() {
        let (control, realtime) = channel::<Ask>(1);
        drop(realtime);
        assert_eq!(
            control.try_reserve().err(),
            Some(ReserveError::Disconnected)
        );
    }

    #[test]
    fn the_realtime_end_receives_the_request_sent() {
        let (control, mut realtime) = channel::<Ask>(1);
        let _handle = open(&control, 7);
        assert!(matches!(
            realtime.try_recv(),
            Some(Received::Request(_, Ask(7)))
        ));
        assert!(realtime.try_recv().is_none());
    }

    #[test]
    fn try_wait_is_empty_until_the_reply_arrives() {
        let (control, mut realtime) = channel::<Ask>(1);
        let mut handle = open(&control, 1);
        assert_eq!(handle.try_wait(), Err(TryWaitError::Empty));
        close_next(&mut realtime);
        assert_eq!(handle.try_wait(), Ok(1));
    }

    #[test]
    fn a_reply_is_taken_once() {
        let (control, mut realtime) = channel::<Ask>(1);
        let mut handle = open(&control, 1);
        close_next(&mut realtime);
        assert_eq!(handle.try_wait(), Ok(1));
        assert_eq!(handle.try_wait(), Err(TryWaitError::Taken));
    }

    #[test]
    fn each_reply_reaches_its_own_handle() {
        let (control, mut realtime) = channel::<Ask>(2);
        let mut first = open(&control, 1);
        let mut second = open(&control, 2);
        let first_permit = receive(&mut realtime);
        let second_permit = receive(&mut realtime);
        realtime.close(second_permit, 20);
        realtime.close(first_permit, 10);
        assert_eq!(first.try_wait(), Ok(10));
        assert_eq!(second.try_wait(), Ok(20));
    }

    #[test]
    fn a_reply_that_came_before_the_realtime_end_went_is_still_taken() {
        let (control, mut realtime) = channel::<Ask>(2);
        let mut answered = open(&control, 1);
        let mut unanswered = open(&control, 2);
        close_next(&mut realtime);
        drop(realtime);
        assert_eq!(answered.try_wait(), Ok(1));
        assert_eq!(unanswered.try_wait(), Err(TryWaitError::Disconnected));
    }

    #[test]
    fn a_dropped_handles_reply_is_freed_on_the_control_side() {
        let (control, mut realtime) = channel::<Hold>(1);
        let held = Arc::new(());
        drop(control.try_reserve().unwrap().open(Hold));
        let Some(Received::Request(permit, Hold)) = realtime.try_recv() else {
            panic!("no request was waiting");
        };
        realtime.close(permit, Arc::clone(&held));
        assert_eq!(Arc::strong_count(&held), 2, "on the ring, not yet freed");

        control.take_in();

        assert_eq!(Arc::strong_count(&held), 1);
    }

    #[test]
    fn cancel_sends_once() {
        let (control, mut realtime) = channel::<Ask>(1);
        let mut handle = open(&control, 1);
        handle.cancel();
        handle.cancel();
        let id = receive(&mut realtime).id();
        assert!(
            matches!(realtime.try_recv(), Some(Received::Cancel(cancelled)) if cancelled == id)
        );
        assert!(realtime.try_recv().is_none());
    }

    #[test]
    fn cancel_after_the_reply_is_taken_sends_nothing() {
        let (control, mut realtime) = channel::<Ask>(1);
        let mut handle = open(&control, 1);
        close_next(&mut realtime);
        assert_eq!(handle.try_wait(), Ok(1));
        handle.cancel();
        assert!(realtime.try_recv().is_none());
    }

    #[test]
    fn a_question_is_answered_once() {
        let (control, mut realtime) = channel::<Ask>(1);
        let mut handle = open(&control, 1);
        let mut status = handle.request_status().unwrap();
        let id = receive(&mut realtime).id();
        let asked = question(&mut realtime);
        assert_eq!(asked.id(), id);
        assert_eq!(status.try_recv(), Err(TryRecvStatusError::Empty));

        realtime.answer(asked, 5);

        assert_eq!(status.try_recv(), Ok(5));
        assert_eq!(status.try_recv(), Err(TryRecvStatusError::Taken));
    }

    #[test]
    fn one_question_is_out_at_a_time() {
        let (control, mut realtime) = channel::<Ask>(1);
        let mut handle = open(&control, 1);
        let mut status = handle.request_status().unwrap();
        assert_eq!(handle.request_status().err(), Some(StatusError::Busy));

        let _permit = receive(&mut realtime);
        let asked = question(&mut realtime);
        realtime.answer(asked, 5);
        assert_eq!(status.try_recv(), Ok(5));

        assert!(handle.request_status().is_ok());
    }

    #[test]
    fn a_dropped_question_holds_until_its_answer_arrives() {
        let (control, mut realtime) = channel::<Ask>(1);
        let mut handle = open(&control, 1);
        drop(handle.request_status().unwrap());
        assert_eq!(handle.request_status().err(), Some(StatusError::Busy));

        let _permit = receive(&mut realtime);
        let asked = question(&mut realtime);
        realtime.answer(asked, 5);

        assert!(handle.request_status().is_ok());
    }

    #[test]
    fn asking_once_the_reply_has_come_is_closed() {
        let (control, mut realtime) = channel::<Ask>(1);
        let mut handle = open(&control, 1);
        close_next(&mut realtime);
        assert_eq!(handle.request_status().err(), Some(StatusError::Closed));
    }

    #[test]
    fn a_question_its_request_closes_before_comes_back_closed() {
        let (control, mut realtime) = channel::<Ask>(1);
        let mut handle = open(&control, 1);
        let mut status = handle.request_status().unwrap();
        close_next(&mut realtime);
        // Dropped unanswered, as the closed request is no longer held.
        let _ = question(&mut realtime);
        assert_eq!(status.try_recv(), Err(TryRecvStatusError::Closed));
    }

    #[test]
    fn asking_is_disconnected_once_the_realtime_end_is_gone() {
        let (control, realtime) = channel::<Ask>(1);
        let mut handle = open(&control, 1);
        drop(realtime);
        assert_eq!(
            handle.request_status().err(),
            Some(StatusError::Disconnected)
        );
    }
}
