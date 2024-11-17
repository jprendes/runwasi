use tokio::sync::watch::{channel, Receiver, Sender};

/// A cell where we can wait (with timeout) for
/// a value to be set
pub struct WaitableCell<T> {
    tx: Sender<Option<T>>,
    rx: Receiver<Option<T>>,
}

impl<T> Default for WaitableCell<T> {
    fn default() -> Self {
        let (tx, rx) = channel(None);
        Self { tx, rx }
    }
}

impl<T> Clone for WaitableCell<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            rx: self.rx.clone(),
        }
    }
}

impl<T> WaitableCell<T> {
    /// Creates an empty WaitableCell.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets a value to the WaitableCell.
    /// This method has no effect if the WaitableCell already has a value.
    pub fn set(&self, val: impl Into<T>) -> Result<(), T> {
        let mut val: Option<T> = Some(val.into());
        self.tx.send_if_modified(|store| {
            if store.is_some() {
                return false;
            }
            std::mem::swap(store, &mut val);
            true
        });
        match val {
            None => Ok(()),
            Some(val) => Err(val),
        }
    }

    /// If the `WaitableCell` is empty when this guard is dropped, the cell will be set to result of `f`.
    /// ```
    /// # use containerd_shim_wasm::sandbox::sync::WaitableCell;
    /// let cell = WaitableCell::<i32>::new();
    /// {
    ///     let _guard = cell.set_guard_with(|| 42);
    /// }
    /// assert_eq!(&42, cell.wait());
    /// ```
    ///
    /// The operation is a no-op if the cell conbtains a value before the guard is dropped.
    /// ```
    /// # use containerd_shim_wasm::sandbox::sync::WaitableCell;
    /// let cell = WaitableCell::<i32>::new();
    /// {
    ///     let _guard = cell.set_guard_with(|| 42);
    ///     let _ = cell.set(24);
    /// }
    /// assert_eq!(&24, cell.wait());
    /// ```
    ///
    /// The function `f` will always be called, regardless of whether the `WaitableCell` has a value or not.
    /// The `WaitableCell` is going to be set even in the case of an unwind. In this case, ff the function `f`
    /// panics it will cause an abort, so it's recommended to avoid any panics in `f`.
    pub fn set_guard_with<R: Into<T>>(&self, f: impl FnOnce() -> R) -> impl Drop {
        let cell = self.clone();
        WaitableCellSetGuard { f: Some(f), cell }
    }

    /// Wait for the WaitableCell to be set a value.
    pub async fn wait(&self) -> &T {
        let mut rx = self.rx.clone();
        let reference = rx.wait_for(|store| store.is_some()).await.unwrap();

        let ptr = reference.as_ref().unwrap() as *const T;

        // this is safe because we never change the value once we set it
        unsafe { &*ptr }
    }
}

// This is the type returned by `WaitableCell::set_guard_with`.
// The public API has no visibility over this type, other than it implements `Drop`
// If the `WaitableCell` `cell`` is empty when this guard is dropped, it will set it's value with the result of `f`.
struct WaitableCellSetGuard<T, R: Into<T>, F: FnOnce() -> R> {
    f: Option<F>,
    cell: WaitableCell<T>,
}

impl<T, R: Into<T>, F: FnOnce() -> R> Drop for WaitableCellSetGuard<T, R, F> {
    fn drop(&mut self) {
        let _ = self.cell.set(self.f.take().unwrap()());
    }
}

#[cfg(test)]
mod test {
    use std::thread::{sleep, spawn};
    use std::time::Duration;

    use super::WaitableCell;
    use crate::sandbox::utils::WithTimeout as _;

    #[tokio::test(flavor = "current_thread")]
    async fn basic() {
        let cell = WaitableCell::<i32>::new();
        cell.set(42).unwrap();
        assert_eq!(&42, cell.wait().await);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn basic_timeout_zero() {
        let cell = WaitableCell::<i32>::new();
        cell.set(42).unwrap();
        assert_eq!(Some(&42), cell.wait().with_timeout(Duration::ZERO).await);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn basic_timeout_1ms() {
        let cell = WaitableCell::<i32>::new();
        cell.set(42).unwrap();
        assert_eq!(
            Some(&42),
            cell.wait().with_timeout(Duration::from_secs(1)).await
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn basic_timeout_none() {
        let cell = WaitableCell::<i32>::new();
        cell.set(42).unwrap();
        assert_eq!(Some(&42), cell.wait().with_timeout(None).await);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unset_timeout_zero() {
        let cell = WaitableCell::<i32>::new();
        assert_eq!(None, cell.wait().with_timeout(Duration::ZERO).await);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unset_timeout_1ms() {
        let cell = WaitableCell::<i32>::new();
        assert_eq!(
            None,
            cell.wait().with_timeout(Duration::from_millis(1)).await
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn clone() {
        let cell = WaitableCell::<i32>::new();
        let cloned = cell.clone();
        let _ = cloned.set(42);
        assert_eq!(&42, cell.wait().await);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn basic_threaded() {
        let cell = WaitableCell::<i32>::new();
        {
            let cell = cell.clone();
            spawn(move || {
                sleep(Duration::from_millis(1));
                let _ = cell.set(42);
            });
        }
        assert_eq!(&42, cell.wait().await);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn basic_double_set() {
        let cell = WaitableCell::<i32>::new();
        assert_eq!(Ok(()), cell.set(42));
        assert_eq!(Err(24), cell.set(24));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn guard() {
        let cell = WaitableCell::<i32>::new();
        {
            let _guard = cell.set_guard_with(|| 42);
        }
        assert_eq!(&42, cell.wait().await);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn guard_no_op() {
        let cell = WaitableCell::<i32>::new();
        {
            let _guard = cell.set_guard_with(|| 42);
            let _ = cell.set(24);
        }
        assert_eq!(&24, cell.wait().await);
    }
}
