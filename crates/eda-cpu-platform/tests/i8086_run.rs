use eda_cpu_platform::i8086::{I8086Machine, I8086Stop, REG_AX, SEG_CS};

#[test]
fn loads_and_runs_a_minimal_mz_executable() {
    let mut image = vec![0; 0x20];
    image[0] = b'M';
    image[1] = b'Z';
    put_u16(&mut image, 2, 0x24); // last-page byte count
    put_u16(&mut image, 4, 1); // one 512-byte page
    put_u16(&mut image, 6, 0); // no relocations
    put_u16(&mut image, 8, 2); // two header paragraphs
    put_u16(&mut image, 0x14, 0); // IP
    put_u16(&mut image, 0x16, 0); // CS relative to loaded image
    image.extend_from_slice(&[
        0xb8, 0x2a, 0x00, // mov ax, 42
        0xf4, // hlt
    ]);
    assert_eq!(&image[0x14..0x16], &[0, 0]);
    let mut machine = I8086Machine::mz(&image, 0x1000).expect("load MZ");
    assert_eq!(machine.segments[SEG_CS], 0x1002);
    assert_eq!(machine.ip, 0);
    assert_eq!(machine.run(100), I8086Stop::Halt);
    assert_eq!(machine.regs[REG_AX], 42);
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
