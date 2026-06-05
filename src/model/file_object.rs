use std::cell::{Cell, RefCell};

use glib::subclass::prelude::*;
use gtk::glib;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct FileObject {
        pub path: RefCell<String>,
        pub size: Cell<u64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FileObject {
        const NAME: &'static str = "RillFileObject";
        type Type = super::FileObject;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for FileObject {}
}

glib::wrapper! {
    pub struct FileObject(ObjectSubclass<imp::FileObject>);
}

impl FileObject {
    pub fn new(path: &str, size: u64) -> Self {
        let obj: Self = glib::Object::builder().build();
        *obj.imp().path.borrow_mut() = path.to_string();
        obj.imp().size.set(size);
        obj
    }

    pub fn path(&self) -> String {
        self.imp().path.borrow().clone()
    }

    pub fn size(&self) -> u64 {
        self.imp().size.get()
    }
}
