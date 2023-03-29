use std::sync::Arc;

use crate::data_source::DataSource;


pub const PAGE_SIZE: usize = 4096;
pub const VADDR_MAX: usize = (1 << 38) - 1; // 2^39 is RISC target. 2^27-1 VA's. Each VA is 9 * 3 VPN bit offsets, and 12 page offset bits..*

type VirtualAddress = usize;

struct MapEntry<'a> {
    source: Arc<dyn DataSource + 'a>,
    offset: usize,
    span: usize,
    addr: usize,
    flags: FlagBuilder
}

impl<'a> MapEntry<'a> {
    #[must_use] // <- not using return value of "new" doesn't make sense, so warn
    pub fn new(source: Arc<dyn DataSource + 'a>, offset: usize, span: usize, addr: usize, flags: FlagBuilder) -> MapEntry<'a> {
        MapEntry {
            source: source.clone(),
            offset,
            span,
            addr,
            flags,
        }
    }
}


/// An address space. Can't live longer than the MapEntries in it?
pub struct AddressSpace<'b>{
    name: String,
    mappings: Vec<MapEntry<'b>>, // see below for comments
}

// comments about storing mappings
// Most OS code uses doubly-linked lists to store sparse data structures like
// an address space's mappings.
// Using Rust's built-in LinkedLists is fine. See https://doc.rust-lang.org/std/collections/struct.LinkedList.html
// But if you really want to get the zen of Rust, this is a really good read, written by the original author
// of that very data structure: https://rust-unofficial.github.io/too-many-lists/

// So, feel free to come up with a different structure, either a classic Rust collection,
// from a crate (but remember it needs to be #no_std compatible), or even write your own.
// See this ticket from Riley: https://github.com/dylanmc/cs393_vm_api/issues/10

impl<'c> AddressSpace<'c> {
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            mappings: Vec::new(), // <- here I changed from LinkedList, for reasons
        } // I encourage you to try other sparse representations - trees, DIY linked lists, ...
    }

    /// Add a mapping from a `DataSource` into this `AddressSpace`.
    ///
    /// # Errors
    /// If the desired mapping is invalid.
    /// TODO: how does our test in lib.rs succeed?
    /// ANSWER: The test in lib.rs makes two mapppings — one at addr 0 of span 1, the other at addr PAGE_SIZE with span 0.
    /// The test asserts that the first mapping doesn't return 0, which is true because we return addr_iter + PAGE_SIZE, 
    /// which = 2*PAGE_SIZE when offset is 0 and span is 1. The second mapping 
    // pub fn add_mapping<'a, D: DataSource + 'a>(
    //     &'a mut self,
    pub fn add_mapping<D: DataSource + 'c>(
        &mut self,
        source: Arc<D>,
        offset: usize,
        span: usize,
        flags: FlagBuilder,
    ) -> Result<VirtualAddress, &str> {
        let mut addr_iter = PAGE_SIZE; // let's not map page 0. addr_iter our running placeholder for where there might be space in the memory.
        let mut gap;
        for mapping in &self.mappings { // look to the next mapping
            gap = mapping.addr - addr_iter; // difference between next mapping & current empty space
            if gap > span + 2 * PAGE_SIZE { // can fit this mapping (span) with empty page each side
                break;
            }
            addr_iter = mapping.addr + mapping.span; // couldn't fit between current guess and this mapping, try next guess at the end of this mapping
            // ROUND UP TO THE NEAREST PAGE
            if addr_iter % PAGE_SIZE != 0 {
                let multiples: usize = addr_iter / PAGE_SIZE;
                addr_iter = (multiples + 1) * PAGE_SIZE;
            }
        }
        if addr_iter + span + 2 * PAGE_SIZE < VADDR_MAX { // 1 blank page on either side. Span for how much this mapping needs. addr_iter for where it can go
            let mapping_addr = addr_iter + PAGE_SIZE; // 1 blank page before.
            let new_mapping: MapEntry = MapEntry::new(source, offset, span, mapping_addr, flags);
            self.mappings.push(new_mapping); // add new mapping to end
            self.mappings.sort_by(|a, b| a.addr.cmp(&b.addr)); // put it in order of addresses
            return Ok(mapping_addr); // no error, result type of usize (called VirtualAddress)
        }
        Err("out of address space!")
    }

    /// Add a mapping from `DataSource` into this `AddressSpace` starting at a specific address.
    ///
    /// # Errors
    /// If there is insufficient room subsequent to `start`.
    pub fn add_mapping_at<D: DataSource + 'c>(
        &mut self,
        source: Arc<D>,
        offset: usize,
        span: usize,
        start: VirtualAddress,
        flags: FlagBuilder
    ) -> Result<(), &str> {
        // check whether there's space for mapping
        let mut next_mapping: usize = 0;
        for mapping in &self.mappings {
            next_mapping = mapping.addr;
            if next_mapping > start {
                break;
            }
        }
        if start + span + 2*PAGE_SIZE < next_mapping {  // there's space! 
            let new_mapping: MapEntry = MapEntry::new(source, offset, span, start, flags);
            self.mappings.push(new_mapping); // add new mapping to end
            self.mappings.sort_by(|a, b| a.addr.cmp(&b.addr)); // put it in order of addresses
            Ok(())
        } else {
            Err("Not enough space after 'start' to map here.")
        }

    }

    /// Remove the mapping to `DataSource` that starts at the given address.
    ///
    /// # Errors
    /// If the mapping could not be removed.
    pub fn remove_mapping<D: DataSource>(
        &mut self,
        source: Arc<D>,
        start: VirtualAddress,
    ) -> Result<(), &str> {
        // iterate through mappings, find the given address? remove that mapping?
        for (mapping_num,mapping) in (&self.mappings).iter().enumerate() {
            if mapping.addr == start {
                self.mappings.remove(mapping_num);
                return Ok(());
            }
        }
        Err("no mapping found starting at that address.")
    }

    /// Look up the DataSource and offset within that DataSource by a
    /// VirtualAddress / AccessType in this AddressSpace
    ///
    /// # Errors
    /// If this VirtualAddress does not have a valid mapping in &self,
    /// or if this AccessType is not permitted by the mapping
    pub fn get_source_for_addr<D: DataSource>(
        &self,
        addr: VirtualAddress,
        access_type: FlagBuilder,
    ) -> Result<(Arc<dyn DataSource + 'c>, usize), &str> {
        for mapping in &self.mappings {
            if mapping.addr == addr {
                // if access_type not one of the flags in mapping.flags. Err
                if mapping.flags.check_access_perms(access_type) {
                    return Ok((mapping.source.clone(), mapping.offset)); // lifetime bug! why does returning a cloned &MapEntry require Address Space to outlive static?
                    // PROBLEM: Address Space, with lifetime 'a, serves a public function that returns an Arc to a Data Source
                    // Rust is worried that returning the Arc to the Data Source will create a dangling reference.
                    // dangling reference or double de-allocate?
                    // CURRENT LIFETIME BOUNDS:
                    // Map Entry cannot outlive internal Data Source
                    // Address Space cannot outlive internal Map Entries
                    // why then it is a problem for a data source to outlive address space?
                }
            }
        }
        todo!()
    }

    /// Helper function for looking up mappings - I don't use...
    fn get_mapping_for_addr(&self, addr: VirtualAddress) -> Result<&MapEntry, &str> {
        for (mapping_num, mapping) in (&self.mappings).iter().enumerate() {
            if mapping_num == addr {
                return Ok(mapping)
            }
        }
        Err("no mapping found at that address")
    }
}

/// Build flags for address space maps.
///
/// We recommend using this builder type as follows:
/// ```
/// # use reedos_address_space::FlagBuilder;
/// let flags = FlagBuilder::new()
///     .toggle_read()
///     .toggle_write();
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // clippy is wrong: bools are more readable than enums
                                         // here because these directly correspond to yes/no
                                         // hardware flags
pub struct FlagBuilder {
    // TODO: should there be some sanity checks that conflicting flags are never toggled? can we do
    // this at compile-time? (the second question is maybe hard)
    read: bool,
    write: bool,
    execute: bool,
    cow: bool,
    private: bool,
    shared: bool,
}

impl FlagBuilder {
    pub fn check_access_perms(&self, access_perms: FlagBuilder) -> bool {
        if access_perms.read && !self.read || access_perms.write && !self.write || access_perms.execute && !self.execute {
            return false;
        }    
        true    
    }

    pub fn is_valid(&self) -> bool {
        if self.private && self.shared {
            return false;
        }
        if self.cow && self.write { // for COW to work, write needs to be off until after the copy
            return false;
        }
        return true;
    }
}
/// Create a constructor and toggler for a `FlagBuilder` object. Will capture attributes, including documentation
/// comments and apply them to the generated constructor.
macro_rules! flag {
    (
        $flag:ident,
        $toggle:ident
    ) => {
        #[doc=concat!("Turn on only the ", stringify!($flag), " flag.")]
        #[must_use]
        pub fn $flag() -> Self {
            Self {
                $flag: true,
                ..Self::default()
            }
        }

        #[doc=concat!("Toggle the ", stringify!($flag), " flag.")]
        #[must_use]
        pub const fn $toggle(self) -> Self {
            Self {
                $flag: !self.$flag,
                ..self
            }
        }
    };
}

impl FlagBuilder {
    /// Create a new `FlagBuilder` with all flags toggled off.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    flag!(read, toggle_read);
    flag!(write, toggle_write);
    flag!(execute, toggle_execute);
    flag!(cow, toggle_cow);
    flag!(private, toggle_private);
    flag!(shared, toggle_shared);

    #[must_use]
    /// Combine two `FlagBuilder`s by boolean or-ing each of their flags.
    ///
    /// This is, somewhat counter-intuitively, named `and`, so that the following code reads
    /// correctly:
    ///
    /// ```
    /// # use reedos_address_space::FlagBuilder;
    /// let read = FlagBuilder::read();
    /// let execute = FlagBuilder::execute();
    /// let new = read.and(execute);
    /// assert_eq!(new, FlagBuilder::new().toggle_read().toggle_execute());
    /// ```
    pub const fn and(self, other: Self) -> Self {
        let read = self.read || other.read;
        let write = self.write || other.write;
        let execute = self.execute || other.execute;
        let cow = self.cow || other.cow;
        let private = self.private || other.private;
        let shared = self.shared || other.shared;

        Self {
            read,
            write,
            execute,
            cow,
            private,
            shared,
        }
    }

    #[must_use]
    /// Turn off all flags in self that are on in other.
    ///
    /// You can think of this as `self &! other` on each field.
    ///
    /// ```
    /// # use reedos_address_space::FlagBuilder;
    /// let read_execute = FlagBuilder::read().toggle_execute();
    /// let execute = FlagBuilder::execute();
    /// let new = read_execute.but_not(execute);
    /// assert_eq!(new, FlagBuilder::new().toggle_read());
    /// ```
    pub const fn but_not(self, other: Self) -> Self {
        let read = self.read && !other.read;
        let write = self.write && !other.write;
        let execute = self.execute && !other.execute;
        let cow = self.cow && !other.cow;
        let private = self.private && !other.private;
        let shared = self.shared && !other.shared;

        Self {
            read,
            write,
            execute,
            cow,
            private,
            shared,
        }
    }
}
