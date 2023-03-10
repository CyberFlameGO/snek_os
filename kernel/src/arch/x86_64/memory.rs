use bootloader_api::info::{MemoryRegion, MemoryRegionKind, MemoryRegions};
use spin::Mutex;
use x86_64::{
    structures::paging::{
        page::PageRange, FrameAllocator, Mapper, OffsetPageTable, Page, PageSize, PageTable,
        PageTableFlags, PhysFrame, Size4KiB, Translate,
    },
    PhysAddr, VirtAddr,
};

use os_units::{Bytes, NumOfPages};

lazy_static! {
    pub static ref MAPPER: Mutex<Option<OffsetPageTable<'static>>> = Mutex::new(None);
    pub static ref FRAME_ALLOCATOR: Mutex<Option<BootInfoFrameAllocator>> = Mutex::new(None);
}

pub unsafe fn init(physical_memory_offset: u64, memory_regions: &'static mut MemoryRegions) {
    let level_4_table = active_level_4_table(physical_memory_offset);
    let table = OffsetPageTable::new(level_4_table, VirtAddr::new(physical_memory_offset));

    let _ = MAPPER.lock().insert(table);

    let frame_allocator = unsafe { BootInfoFrameAllocator::init(memory_regions) };

    let _ = FRAME_ALLOCATOR.lock().insert(frame_allocator);

    println!("[MEMORY] initialized");
}

unsafe fn active_level_4_table(physical_memory_offset: u64) -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;

    let (level_4_table_frame, _) = Cr3::read();

    let phys = level_4_table_frame.start_address();
    let virt = VirtAddr::new(physical_memory_offset + phys.as_u64());
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    &mut *page_table_ptr // unsafe
}

pub struct BootInfoFrameAllocator {
    memory_regions: &'static mut [MemoryRegion],
    next: usize,
}

impl BootInfoFrameAllocator {
    pub unsafe fn init(memory_regions: &'static mut MemoryRegions) -> Self {
        BootInfoFrameAllocator {
            memory_regions,
            next: 0,
        }
    }

    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> + '_ {
        let usable_regions = self
            .memory_regions
            .iter()
            .filter(|r| r.kind == MemoryRegionKind::Usable);
        // map each region to its address range
        let addr_ranges = usable_regions.map(|r| r.start..r.end);
        // transform to an iterator of frame start addresses
        let frame_addresses = addr_ranges.flat_map(|r| r.step_by(4096));
        // create `PhysFrame` types from the start addresses
        frame_addresses.map(|addr| PhysFrame::containing_address(PhysAddr::new(addr)))
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let frame = self.usable_frames().nth(self.next);
        self.next += 1;
        frame
    }
}

fn search_free_addr_from(num_pages: NumOfPages<Size4KiB>, region: PageRange) -> Option<VirtAddr> {
    let mut cnt = 0;
    let mut start = None;
    for page in region {
        let addr = page.start_address();
        if available(addr) {
            if start.is_none() {
                start = Some(addr);
            }

            cnt += 1;

            if cnt >= num_pages.as_usize() {
                return start;
            }
        } else {
            cnt = 0;
            start = None;
        }
    }

    None
}

fn available(addr: VirtAddr) -> bool {
    let mut binding = super::memory::MAPPER.lock();
    let mapper = binding.as_mut().unwrap();

    mapper.translate_addr(addr).is_none() && !addr.is_null()
}

pub fn map_pages_from(start: PhysAddr, object_size: usize, region: PageRange) -> VirtAddr {
    let start_frame_addr = start.align_down(Size4KiB::SIZE);
    let end_frame_addr = (start + object_size).align_down(Size4KiB::SIZE);

    let num_pages =
        Bytes::new((end_frame_addr - start_frame_addr) as usize + 1).as_num_of_pages::<Size4KiB>();

    let virt = search_free_addr_from(num_pages, region)
        .expect("OOM during creating a new accessor to a register.");

    let mut mapper = super::memory::MAPPER.lock();
    let mapper = mapper.as_mut().unwrap();

    let mut frame_allocator = super::memory::FRAME_ALLOCATOR.lock();
    let frame_allocator = frame_allocator.as_mut().unwrap();

    for i in 0..num_pages.as_usize() {
        let page = Page::<Size4KiB>::containing_address(virt + Size4KiB::SIZE * i as u64);
        let frame = PhysFrame::containing_address(start_frame_addr + Size4KiB::SIZE * i as u64);
        let flag =
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

        unsafe {
            mapper
                .map_to(page, frame, flag, frame_allocator)
                .unwrap()
                .flush();
        }
    }

    let page_offset = start.as_u64() % Size4KiB::SIZE;

    virt + page_offset
}

pub fn map_address(phys: PhysAddr, size: usize) -> VirtAddr {
    map_pages_from(
        phys,
        size,
        PageRange {
            start: Page::from_start_address(VirtAddr::new(0xFFFF_8000_0000_0000)).unwrap(),
            end: Page::containing_address(VirtAddr::new(0xFFFF_FFFF_FFFF_FFFF)),
        },
    )
}
