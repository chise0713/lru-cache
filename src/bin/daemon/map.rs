use std::{collections::BTreeMap, path::Path};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct AtimeSize {
    atime: i64,
    size: u64,
}

impl AtimeSize {
    #[inline(always)]
    fn size(&self) -> u64 {
        self.size
    }
}

pub struct PathAtimeSizeMap {
    map: BTreeMap<Box<Path>, AtimeSize>,
    total_size: u64,
}

impl PathAtimeSizeMap {
    pub fn new() -> Self {
        Self {
            map: BTreeMap::default(),
            total_size: 0,
        }
    }

    pub fn insert<P: AsRef<Path>>(&mut self, path: P, atime: i64, size: u64) {
        let path = path.as_ref();
        let previous_size = self.map.get(path).map(AtimeSize::size).unwrap_or_default();
        self.map.insert(Box::from(path), AtimeSize { atime, size });
        self.total_size = self.total_size - previous_size + size;
        dbg!(self.total_size, size);
    }

    pub fn remove<P: AsRef<Path>>(&mut self, path: P) {
        let path = path.as_ref();
        let size = self
            .map
            .remove(path)
            .as_ref()
            .map(AtimeSize::size)
            .unwrap_or_default();
        self.total_size -= size;
        dbg!(self.total_size, size);
    }

    pub fn plan_evict_until(
        &self,
        target_size: u64,
        prefix_filter: Option<&Path>,
    ) -> impl Iterator<Item = &Path> + '_ {
        #[expect(clippy::borrowed_box)]
        fn map_box_as_ref<'a>(
            (path, atime_size): (&'a Box<Path>, &'a AtimeSize),
        ) -> (&'a Path, &'a AtimeSize) {
            (path.as_ref(), atime_size)
        }

        (self.total_size > target_size)
            .then(|| {
                let mut entries: Box<[_]> = match prefix_filter {
                    None => self.map.iter().map(map_box_as_ref).collect(),
                    Some(prefix) => self
                        .map
                        .iter()
                        .filter(|(path, _)| path.starts_with(prefix))
                        .map(map_box_as_ref)
                        .collect(),
                };

                entries.sort_unstable_by_key(|(path, atime_size)| (*atime_size, *path));

                let cutoff = entries
                    .iter()
                    .scan(self.total_size, |remaining, (_, AtimeSize { size, .. })| {
                        *remaining = remaining.saturating_sub(*size);
                        Some(*remaining)
                    })
                    .position(|remaining| remaining <= target_size)
                    .map(|i| i + 1)
                    .unwrap_or(entries.len());

                entries.into_iter().take(cutoff).map(|(path, _)| path)
            })
            .into_iter()
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_total_size_tracks_insert_and_remove() {
        let mut map = PathAtimeSizeMap::new();

        map.insert("/a", 1, 10);
        map.insert("/b", 2, 20);

        assert_eq!(map.total_size, 30);

        map.remove("/a");
        assert_eq!(map.total_size, 20);

        map.remove("/not-exist");
        assert_eq!(map.total_size, 20);
    }

    #[test]
    fn test_insert_same_path_updates_size() {
        let mut map = PathAtimeSizeMap::new();

        map.insert("/a", 1, 10);
        map.insert("/a", 2, 25);

        assert_eq!(map.total_size, 25);
    }

    #[test]
    fn test_no_evict_when_under_target() {
        let mut map = PathAtimeSizeMap::new();

        map.insert("/a", 1, 10);
        map.insert("/b", 2, 20);

        let evicted: Box<[_]> = map.plan_evict_until(30, None).collect();
        assert!(evicted.is_empty());
    }

    #[test]
    fn evict_oldest_atime_first() {
        let mut map = PathAtimeSizeMap::new();

        map.insert("/old", 1, 10);
        map.insert("/mid", 2, 10);
        map.insert("/new", 3, 10);

        let evicted: Box<[_]> = map.plan_evict_until(15, None).collect();

        assert_eq!(evicted.as_ref(), &[Path::new("/old"), Path::new("/mid")]);
    }
}
