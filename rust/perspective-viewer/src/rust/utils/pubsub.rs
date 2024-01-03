// ┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
// ┃ ██████ ██████ ██████       █      █      █      █      █ █▄  ▀███ █       ┃
// ┃ ▄▄▄▄▄█ █▄▄▄▄▄ ▄▄▄▄▄█  ▀▀▀▀▀█▀▀▀▀▀ █ ▀▀▀▀▀█ ████████▌▐███ ███▄  ▀█ █ ▀▀▀▀▀ ┃
// ┃ █▀▀▀▀▀ █▀▀▀▀▀ █▀██▀▀ ▄▄▄▄▄ █ ▄▄▄▄▄█ ▄▄▄▄▄█ ████████▌▐███ █████▄   █ ▄▄▄▄▄ ┃
// ┃ █      ██████ █  ▀█▄       █ ██████      █      ███▌▐███ ███████▄ █       ┃
// ┣━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┫
// ┃ Copyright (c) 2017, the Perspective Authors.                              ┃
// ┃ ╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌ ┃
// ┃ This file is part of the Perspective library, distributed under the terms ┃
// ┃ of the [Apache License 2.0](https://www.apache.org/licenses/LICENSE-2.0). ┃
// ┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use derivative::Derivative;
use futures::channel::oneshot::*;
use yew::prelude::*;

/// An internal `HashSet` variant which supports unconstrained `T` e.g.
/// without `Hash`, via returning a unique `usize` index for each insert
/// which can be used for a reciprocal `remove(x: usize)`.
#[derive(Derivative)]
#[derivative(Default(bound = ""))]
struct IndexedSet<T> {
    set: HashMap<usize, T>,
    gen: usize,
}

impl<T> IndexedSet<T> {
    fn insert(&mut self, v: T) -> usize {
        let key = self.gen;
        self.set.insert(key, v);
        self.gen += 1;
        key
    }

    fn remove(&mut self, key: usize) {
        self.set.remove(&key);
    }

    fn iter(&self) -> impl Iterator<Item = &T> {
        self.set.values()
    }

    fn drain(&mut self) -> impl Iterator<Item = T> {
        let mut x = Box::default();
        std::mem::swap(&mut self.set, &mut x);
        x.into_values()
    }
}

type ListenerSet<T> = IndexedSet<Box<dyn Fn(T) + 'static>>;
type ListenerOnceSet<T> = IndexedSet<Box<dyn FnOnce(T) + 'static>>;

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct PubSubInternal<T: Clone> {
    deleted: Cell<bool>,
    listeners: RefCell<ListenerSet<T>>,
    once_listeners: RefCell<ListenerOnceSet<T>>,
}

impl<T: Clone> PubSubInternal<T> {
    fn emit(&self, val: T) {
        if self.deleted.get() {
            tracing::warn!("`Callback` invoked after `PubSub` dropped");
        }

        for listener in self.listeners.borrow().iter() {
            listener(val.clone());
        }

        for listener in self.once_listeners.borrow_mut().drain() {
            listener(val.clone());
        }
    }
}

/// A pub/sub struct which allows many listeners to subscribe to many
/// publishers, without leaking callbacks as listeners are dropped. Unlike
/// `mpsc` etc., `PubSub` has no internal queue and is completely synchronous.
#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct PubSub<T: Clone>(Rc<PubSubInternal<T>>);

pub trait AddListener<T> {
    /// Register a listener to this `PubSub<_>`, which will be automatically
    /// deregistered when the return `Subscription` is dropped.
    ///
    /// # Arguments
    /// - `f` The callback, presumably a function-like type.
    fn add_listener(&self, f: T) -> Subscription;
}

impl<T: Clone + 'static> PubSub<T> {
    /// Emit a value to all listeners.
    ///
    /// # Arguments
    /// - `val` The value to emit.
    pub fn emit(&self, val: T) {
        self.0.emit(val);
    }

    /// Get this `PubSub<_>`'s `.emit_all()` method as a `Callback<T>`.
    pub fn callback(&self) -> Callback<T> {
        let internal = self.0.clone();
        Callback::from(move |val: T| internal.emit(val))
    }

    /// Await this `PubSub<_>`'s next call to `emit_all()`, once.
    pub async fn listen_once(&self) -> Result<T, Canceled> {
        let (sender, receiver) = channel::<T>();
        let f = move |x| sender.send(x).unwrap_or(());
        self.0.once_listeners.borrow_mut().insert(Box::new(f));
        receiver.await
    }
}

impl<T: Clone> Drop for PubSub<T> {
    fn drop(&mut self) {
        self.0.deleted.set(true);
    }
}

impl<T: Clone + 'static> AddListener<Callback<T>> for PubSub<T> {
    fn add_listener(&self, f: Callback<T>) -> Subscription {
        let internal = self.0.clone();
        let cb = Box::new(move |x| f.emit(x));
        let key = self.0.listeners.borrow_mut().insert(cb);
        Subscription(Box::new(move || {
            internal.listeners.borrow_mut().remove(key)
        }))
    }
}

impl<T, U> AddListener<U> for PubSub<T>
where
    T: Clone + 'static,
    U: Fn(T) + 'static,
{
    fn add_listener(&self, f: U) -> Subscription {
        let internal = self.0.clone();
        let key = self.0.listeners.borrow_mut().insert(Box::new(f));
        Subscription(Box::new(move || {
            internal.listeners.borrow_mut().remove(key)
        }))
    }
}

/// Manages the lifetime of a listener registered to a `PubSub<T>` by
/// deregistering the associated listener when dropped.  The wrapped `Fn` of
/// `Subscriptions` is the deregister closure provided by the issuing
/// `PubSub<T>`.
#[must_use]
pub struct Subscription(Box<dyn Fn()>);

impl Drop for Subscription {
    fn drop(&mut self) {
        (*self.0)();
    }
}
