//! Small typed pages keep payload addresses stable through exclusive growth.

const ROWS_PER_PAGE: usize = 16;

enum Directory<T> {
    Empty,
    One(Box<[T; ROWS_PER_PAGE]>),
    Many(Vec<Box<[T; ROWS_PER_PAGE]>>),
}

pub(crate) struct RowPages<T> {
    directory: Directory<T>,
    len: u32,
}

impl<T> Default for RowPages<T> {
    fn default() -> Self {
        Self {
            directory: Directory::Empty,
            len: 0,
        }
    }
}

impl<T> RowPages<T> {
    pub(crate) fn len(&self) -> u32 {
        self.len
    }

    pub(crate) fn get(&self, ordinal: u32) -> Option<&T> {
        if ordinal >= self.len {
            return None;
        }
        let page = match &self.directory {
            Directory::Empty => return None,
            Directory::One(page) => page,
            Directory::Many(pages) => pages.get(ordinal as usize / ROWS_PER_PAGE)?,
        };
        page.get(ordinal as usize % ROWS_PER_PAGE)
    }

    pub(crate) fn get_mut(&mut self, ordinal: u32) -> Option<&mut T> {
        if ordinal >= self.len {
            return None;
        }
        let page = match &mut self.directory {
            Directory::Empty => return None,
            Directory::One(page) => page,
            Directory::Many(pages) => pages.get_mut(ordinal as usize / ROWS_PER_PAGE)?,
        };
        page.get_mut(ordinal as usize % ROWS_PER_PAGE)
    }
}

impl<T: Default> RowPages<T> {
    pub(crate) fn push(&mut self, value: T) -> u32 {
        let ordinal = self.len;
        let next = ordinal
            .checked_add(1)
            .expect("typed payload ordinal space exhausted");
        if (ordinal as usize).is_multiple_of(ROWS_PER_PAGE) {
            let page = Box::new(std::array::from_fn(|_| T::default()));
            self.directory = match std::mem::replace(&mut self.directory, Directory::Empty) {
                Directory::Empty => Directory::One(page),
                Directory::One(first) => Directory::Many(vec![first, page]),
                Directory::Many(mut pages) => {
                    pages.push(page);
                    Directory::Many(pages)
                }
            };
        }
        self.len = next;
        *self.get_mut(ordinal).expect("newly allocated payload row") = value;
        ordinal
    }
}

impl<T> RowPages<T> {
    /// Reserved row bytes and the page directory's capacity.
    pub(crate) fn structural_bytes(&self) -> usize {
        match &self.directory {
            Directory::Empty => 0,
            Directory::One(_) => ROWS_PER_PAGE * size_of::<T>(),
            Directory::Many(pages) => {
                pages.capacity() * size_of::<Box<[T; ROWS_PER_PAGE]>>()
                    + pages.len() * ROWS_PER_PAGE * size_of::<T>()
            }
        }
    }
}
