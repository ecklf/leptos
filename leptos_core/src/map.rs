use leptos_reactive::{
    create_effect, create_memo, create_signal, queue_microtask, Memo, ReadSignal, Scope,
    ScopeDisposer,
};
use std::{collections::HashMap, fmt::Debug, hash::Hash, ops::IndexMut};

/// Function that maps a `Vec` to another `Vec` via a map function. The mapped `Vec` is lazy
/// computed; its value will only be updated when requested. Modifications to the
/// input `Vec` are diffed using keys to prevent recomputing values that have not changed.
///
/// This function is the underlying utility behind `Keyed`.
///
/// # Params
/// * `list` - The list to be mapped. It is obtained via an accessor function, so can be a ReadSignal, a Memo
///   or a derived signal.
/// * `map_fn` - A closure that maps from the input type to the output type.
/// * `key_fn` - A closure that returns an _unique_ key to each entry.
///
///  _Credits: Based on implementation for [Sycamore](https://github.com/sycamore-rs/sycamore/blob/53735aab9ef72b98439b4d2eaeb85a97f7f32775/packages/sycamore-reactive/src/iter.rs),
/// which is in turned based on on the TypeScript implementation in <https://github.com/solidjs/solid>_
pub fn map_keyed<T, U, K>(
    cx: Scope,
    list: impl Fn() -> Vec<T> + 'static,
    map_fn: impl Fn(Scope, &T) -> U + 'static,
    key_fn: impl Fn(&T) -> K + 'static,
) -> Memo<Vec<U>>
//-> impl FnMut() -> Vec<U>
where
    T: PartialEq + Debug + Clone + 'static,
    K: Eq + Hash,
    U: PartialEq + Debug + Clone + 'static,
{
    // Previous state used for diffing.
    let mut disposers: Vec<Option<ScopeDisposer>> = Vec::new();
    let mut prev_items: Option<Vec<T>> = None;
    //let mapped: Vec<U> = Vec::new();

    // Diff and update signal each time list is updated.
    create_memo(cx, move |mapped: Option<Vec<U>>| {
        let mut mapped = mapped.unwrap_or_default();
        let items = prev_items.take().unwrap_or_default();
        let new_items = list();
        let new_items_len = new_items.len();

        if new_items.is_empty() {
            // Fast path for removing all items.
            let disposers = std::mem::take(&mut disposers);
            // delay disposal until after the current microtask
            queue_microtask(move || {
                for disposer in disposers.into_iter().flatten() {
                    disposer.dispose();
                }
            });
            mapped.clear();
        } else if items.is_empty() {
            // Fast path for creating items when the existing list is empty.
            for new_item in new_items.iter() {
                let mut value: Option<U> = None;
                let new_disposer = cx.child_scope(|cx| {
                    value = Some(map_fn(cx, new_item));
                });
                mapped.push(value.unwrap());
                disposers.push(Some(new_disposer));
            }
        } else {
            let mut temp = vec![None; new_items.len()];
            let mut temp_disposers: Vec<Option<ScopeDisposer>> =
                (0..new_items.len()).map(|_| None).collect();

            // Skip common prefix.
            let min_len = usize::min(items.len(), new_items.len());
            let start = items
                .iter()
                .zip(new_items.iter())
                .position(|(a, b)| a != b)
                .unwrap_or(min_len);

            // Skip common suffix.
            let mut end = items.len();
            let mut new_end = new_items.len();
            #[allow(clippy::suspicious_operation_groupings)]
            // FIXME: make code clearer so that clippy won't complain
            while end > start && new_end > start && items[end - 1] == new_items[new_end - 1] {
                end -= 1;
                new_end -= 1;
                temp[new_end] = Some(mapped[end].clone());
                temp_disposers[new_end] = disposers[end].take();
            }

            // 0) Prepare a map of indices in newItems. Scan backwards so we encounter them in
            // natural order.
            let mut new_indices = HashMap::with_capacity(new_end - start);

            // Indexes for new_indices_next are shifted by start because values at 0..start are
            // always None.
            let mut new_indices_next = vec![None; new_end - start];
            for j in (start..new_end).rev() {
                let item = &new_items[j];
                let i = new_indices.get(&key_fn(item));
                new_indices_next[j - start] = i.copied();
                new_indices.insert(key_fn(item), j);
            }

            // 1) Step through old items and see if they can be found in new set; if so, mark
            // them as moved.
            for i in start..end {
                let item = &items[i];
                if let Some(j) = new_indices.get(&key_fn(item)).copied() {
                    // Moved. j is index of item in new_items.
                    temp[j] = Some(mapped[i].clone());
                    temp_disposers[j] = disposers[i].take();
                    new_indices_next[j - start].and_then(|j| new_indices.insert(key_fn(item), j));
                } else {
                    // Create new.
                    disposers[i].take().unwrap().dispose();
                }
            }

            // 2) Set all the new values, pulling from the moved array if copied, otherwise
            // entering the new value.
            for j in start..new_items.len() {
                if matches!(temp.get(j), Some(Some(_))) {
                    // Pull from moved array.
                    if j >= mapped.len() {
                        mapped.push(temp[j].clone().unwrap());
                        disposers.push(temp_disposers[j].take());
                    } else {
                        *mapped.index_mut(j) = temp[j].clone().unwrap();
                        disposers[j] = temp_disposers[j].take();
                    }
                } else {
                    // Create new value.
                    let mut tmp = None;
                    let new_item = &new_items[j];
                    let new_disposer = cx.child_scope(|cx| {
                        tmp = Some(map_fn(cx, new_item));
                    });
                    if mapped.len() > j {
                        mapped[j] = tmp.unwrap();
                        disposers[j] = Some(new_disposer);
                    } else {
                        mapped.push(tmp.unwrap());
                        disposers.push(Some(new_disposer));
                    }
                }
            }
        }
        // 3) In case the new set is shorter than the old, set the length of the mapped array.
        mapped.truncate(new_items_len);
        disposers.truncate(new_items_len);

        // 4) Return the mapped and new items, for use in next iteration
        prev_items = Some(new_items);

        mapped.to_vec()
    })
}

#[cfg(test)]
mod tests {
    use crate::map::map_keyed;
    use leptos_reactive::*;

    #[test]
    fn test_map_keyed() {
        create_scope(|cx| {
            let (rows, set_rows) =
                create_signal::<Vec<(usize, ReadSignal<i32>, WriteSignal<i32>)>>(cx, vec![]);

            let keyed = map_keyed(
                cx,
                rows,
                |cx, row| {
                    let read = row.1;
                    create_effect(cx, move |_| println!("row value = {}", read.get()));
                },
                |row| row.0,
            );

            create_effect(cx, move |_| println!("keyed = {:#?}", keyed.get()));

            let (r, w) = create_signal(cx, 0);
            set_rows.update(|n| n.push((0, r, w)));
        })
        .dispose();
    }
}
