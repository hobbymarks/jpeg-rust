use std::iter::repeat;

use jpeg::jfif::{FrameHeader, ScanHeader};
use jpeg::huffman;
use ::transform;

type QuantizationTable = Vec<u8>;
type Block = Vec<f32>;

/// Struct to hold state of JPEG decoding.
/// Instantiate it, and pass in AC/DC tables, quantization
/// tables, sampling factors, data, etc. as it is available,
/// or updated.
///
/// Call `JPEGDecoder::decode()` to start reading from `data`.
pub struct JPEGDecoder<'a> {
    /// Encoded image data
    data: &'a [u8],
    /// Huffman tables for AC coefficients
    huffman_ac_tables: [Option<huffman::Table>; 4],
    /// Huffman tables for DC coefficients
    huffman_dc_tables: [Option<huffman::Table>; 4],
    /// Quantization tables
    quantization_tables: [Option<QuantizationTable>; 4],
    /// Fields specific for each component.
    component_fields: Vec<JPEGDecoderComponentFields>,
    /// Image dimensions
    dimensions: (usize, usize),
}

impl<'a> JPEGDecoder<'a> {
    pub fn new(data: &'a [u8]) -> JPEGDecoder {
        JPEGDecoder {
            data: data,
            huffman_ac_tables: [None, None, None, None],
            huffman_dc_tables: [None, None, None, None],
            quantization_tables: [None, None, None, None],
            component_fields: Vec::new(),
            dimensions: (0, 0),
        }
    }

    pub fn dimensions(mut self, dimensions: (usize, usize)) -> JPEGDecoder<'a> {
        self.dimensions = dimensions;
        self
    }

    pub fn huffman_ac_tables(&mut self, id: u8, table: huffman::Table) {
        self.huffman_ac_tables[id as usize] = Some(table);
    }

    pub fn huffman_dc_tables(&mut self, id: u8, table: huffman::Table) {
        self.huffman_dc_tables[id as usize] = Some(table);
    }

    pub fn quantization_table(&mut self, id: u8, table: Vec<u8>) {
        self.quantization_tables[id as usize] = Some(table);
    }

    pub fn frame_header(mut self, frame_header: FrameHeader) -> JPEGDecoder<'a> {
        for frame_component in &frame_header.frame_components {
            // Update horiz/vert sampling factor, and quant selector.
            let was_none = self.component_fields
                .iter_mut()
                .find(|cf| cf.component == frame_component.component_id)
                .as_mut()
                .map(|cf| {
                    cf.horizontal_sampling_factor = frame_component.horizontal_sampling_factor;
                    cf.vertical_sampling_factor = frame_component.vertical_sampling_factor;
                    cf.quantization_id = frame_component.quantization_selector;
                })
                .is_none();
            // Or insert a new element, if none was found.
            if was_none {
                self.component_fields.push({
                    JPEGDecoderComponentFields {
                        component: frame_component.component_id,
                        horizontal_sampling_factor: frame_component.horizontal_sampling_factor,
                        vertical_sampling_factor: frame_component.vertical_sampling_factor,
                        quantization_id: frame_component.quantization_selector,
                        dc_table_id: 0xff,
                        ac_table_id: 0xff,
                    }
                });
            }
        }
        self
    }

    pub fn scan_header(mut self, scan_header: ScanHeader) -> JPEGDecoder<'a> {
        for scan_component in &scan_header.scan_components {
            // Update horiz/vert sampling factor, and quant selector.
            let was_none = self.component_fields
                .iter_mut()
                .find(|cf| cf.component == scan_component.component_id)
                .as_mut()
                .map(|cf| {
                    cf.ac_table_id = scan_component.ac_table_selector;
                    cf.dc_table_id = scan_component.dc_table_selector;
                })
                .is_none();
            // Or insert a new element, if none was found.
            if was_none {
                self.component_fields.push({
                    JPEGDecoderComponentFields {
                        component: scan_component.component_id,
                        horizontal_sampling_factor: 0xff,
                        vertical_sampling_factor: 0xff,
                        quantization_id: 0xff,
                        dc_table_id: scan_component.ac_table_selector,
                        ac_table_id: scan_component.dc_table_selector,
                    }
                });
            }
        }
        // The order of the components is the order from scan_header.
        // Make sure this is the case.
        self.component_fields = scan_header.scan_components
            .iter()
            .map(|scan_component| {
                self.component_fields
                    .iter()
                    .find(|cf| cf.component == scan_component.component_id)
                    .cloned()
                    .unwrap()
            })
            .collect();
        self
    }

    fn ac_table(&'a self, id: u8) -> &'a huffman::Table {
        self.huffman_ac_tables[id as usize].as_ref().unwrap()
    }

    fn dc_table(&'a self, id: u8) -> &'a huffman::Table {
        self.huffman_dc_tables[id as usize].as_ref().unwrap()
    }

    pub fn decode(&mut self) -> () {
        // Number of blocks in x and y direction
        let num_blocks_x = (self.dimensions.0 + 7) / 8;
        let num_blocks_y = (self.dimensions.1 + 7) / 8;
        let num_blocks = num_blocks_x * num_blocks_y;

        let num_components = self.component_fields.len();
        println!("Decoding {}x{} blocks of {} components",
                 num_blocks_x,
                 num_blocks_y,
                 num_components);

        let mut scan_state = huffman::ScanState {
            index: 0,
            bits_read: 0,
        };
        // 2D vector, one vector for each component.
        let mut blocks: Vec<Vec<Block>> = (0..self.component_fields.len())
            .map(|_| vec![])
            .collect();
        let mut previous_dc: Vec<f32> = repeat(0.0).take(self.component_fields.len()).collect();

        // Step 1: Read encoded data
        for block_i in 0..num_blocks {
            for (component_i, component) in self.component_fields.iter().enumerate() {

                // Skip reading components which have a sampling factor,
                // and is not at the last block (eg, factor=2 menas read
                // on odd indices).
                let hsf = component.horizontal_sampling_factor as usize;
                let vsf = component.vertical_sampling_factor as usize;
                if block_i % hsf != (hsf - 1) || (block_i / num_blocks_y) % vsf != (vsf - 1) {
                    continue;
                }

                let ac_table = self.ac_table(component.ac_table_id);
                let dc_table = self.dc_table(component.dc_table_id);

                let mut decoded_block: Vec<f32> =
                    huffman::decode(ac_table, dc_table, &self.data, &mut scan_state)
                        .iter()
                        .map(|&i| i as f32)
                        .collect();

                // DC correction
                let encoded = decoded_block[0];
                decoded_block[0] = encoded + previous_dc[component_i];
                previous_dc[component_i] = decoded_block[0];

                blocks[component_i].push(decoded_block);
            }
        }
        println!("{:?}", blocks[0][0]);

        // Step 2: get color data
        // Now all decoded blocks are in `blocks`.
        // For each block, do dequantization, reverse zigzag, and inverse DCT.
        for (component_i, component) in self.component_fields.iter().enumerate() {
            let quant_table = self.quantization_tables[component.quantization_id as usize]
                .as_ref()
                .expect(&format!("Did not find quantization table for {}",
                                 component.quantization_id));

            let component_blocks: Vec<Vec<f32>> = blocks[component_i]
                .iter()
                .map(|block| {
                    zigzag_inverse(block.iter()
                        .zip(quant_table.iter())
                        .map(|(&n, &q)| n * q as f32))
                })
                .map(|block| transform::discrete_cosine_transform_inverse(&block))
                .collect();

            // Now we may need to expand blocks for some compoents,
            // in case some sampling factors are > 1.
            assert!(component.horizontal_sampling_factor < 3);
            // TODO: Fix vertical scaling
            assert!(component.vertical_sampling_factor == 1);
            if component.horizontal_sampling_factor == 2 {
                blocks[component_i] = component_blocks.iter()
                    .flat_map(|block| {
                        let (a, b) = expand_block_x_2(block);
                        vec![a, b]
                    })
                    .collect();
            } else {
                blocks[component_i] = component_blocks;
            }
        }
        println!("{:?}", blocks[0][0]);

        // Step 3: Merge color data
        let rgb_blocks: Vec<Vec<(u8, u8, u8)>> = if num_components == 3 {
                blocks[0]
                    .iter()
                    .zip(blocks[1].iter())
                    .zip(blocks[2].iter())
                    .map(|((y_block, cb_block), cr_block)| {
                        y_block.iter()
                            .zip(cb_block.iter())
                            .zip(cr_block.iter())
                            .map(|((&y, &cb), &cr)| y_cb_cr_to_rgb(y, cb, cr))
                            .collect()
                    })
                    .collect()
            } else {
                // TODO: find out if grayscale, is it still YCbCr, or just RGB?
                blocks[0]
                    .iter()
                    .map(|block| {
                        block.iter()
                        // .map(|&g| y_cb_cr_to_rgb(g, g, g))
                        .map(|&g| (g, g, g))
                        .collect()
                    })
                    .collect::<Vec<Vec<(f32, f32, f32)>>>()
            }
            .iter()
            .map(|block| {
                block.iter()
                    .map(|&(r, g, b)| {
                        (f32_to_u8(r + 128.0), f32_to_u8(g + 128.0), f32_to_u8(b + 128.0))
                    })
                    .collect()
            })
            .collect();


        // debug print
        for (i, n) in rgb_blocks[0].iter().enumerate() {
            print!("{:02x}{:02x}{:02x} ", n.0, n.1, n.2);
            if i % 8 == 7 {
                print!("\n");
            }
        }

        let mut image_data: Vec<(u8, u8, u8)> = Vec::with_capacity(num_blocks * 64);
        for block_y in 0..num_blocks_y {
            for line in 0..8 {
                for block_x in 0..num_blocks_x {
                    let block_index = num_blocks_x * block_y + block_x;
                    let ref block = rgb_blocks[block_index];
                    for row in 0..8 {
                        let in_block_index = 8 * line + row;
                        image_data.push(block[in_block_index]);
                    }
                }
            }
        }
        // Show the image, somehow.

        use std::fs::File;
        use std::io::Write;
        let mut file = File::create("output.ppm").unwrap();
        let _ =
            file.write(format!("P3\n{} {}\n255\n", 8 * num_blocks_x, 8 * num_blocks_y).as_bytes());
        for &(r, g, b) in &image_data {
            let s = format!("{} {} {}\n", r, g, b);
            let _ = file.write(s.as_bytes());
        }

    }
}

fn f32_to_u8(n: f32) -> u8 {
    if n < 0.0 {
        0
    } else if n > 255.0 {
        255
    } else {
        n as u8
    }
}

fn y_cb_cr_to_rgb(y: f32, cb: f32, cr: f32) -> (f32, f32, f32) {
    let c_red: f32 = 0.299;
    let c_green: f32 = 0.587;
    let c_blue: f32 = 0.114;

    let r = cr * (2.0 - 2.0 * c_red) + y;
    let b = cb * (2.0 - 2.0 * c_blue) + y;
    let g = (y - c_blue * b - c_red * r) / c_green;

    (r, g, b)
}

fn expand_block_x_2(block: &Block) -> (Block, Block) {
    // Expand a block along the x axis:
    //
    //  |1 2|      |1 1| |2 2|
    //  |3 4|  --> |3 3| |4 4|

    // Assume 8x8 block:
    assert!(block.len() == 64,
            "Implement me properly! (len={})",
            block.len());

    let mut block_a = Vec::new();
    let mut block_b = Vec::new();
    for (i, &n) in block.iter().enumerate() {
        let is_block_a = (i % 8) / 2 < 4;
        if is_block_a {
            block_a.push(n);
            block_a.push(n);
        } else {
            block_b.push(n);
            block_b.push(n);
        }
    }
    (block_a, block_b)
}

#[derive(Debug, Clone)]
/// All component specific fields:
///
// TODO: Rather use Option<> on the fields, as they may not
//       be set?
struct JPEGDecoderComponentFields {
    /// Component ID
    component: u8,
    /// AC Huffman table id
    dc_table_id: u8,
    /// DC Huffman table id
    ac_table_id: u8,
    /// Quantization table id
    quantization_id: u8,
    /// Number of pixels for each sample in horizontal direction (?)
    horizontal_sampling_factor: u8,
    /// Number of pixels for each sample in horizontal direction (?)
    vertical_sampling_factor: u8,
}

// hardcode dis shit lol
const ZIGZAG_INDICES: [usize; 64] =
    [0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27,
     20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58,
     59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63];
#[allow(dead_code)]
fn zigzag<T>(vec: &Vec<T>) -> Vec<T>
    where T: Copy
{
    if vec.len() != 64 {
        panic!("I took a shortcut in zigzag()! Please implement me properly :) (len={})",
               vec.len());
    }
    let mut res = Vec::with_capacity(64);
    for &i in ZIGZAG_INDICES.iter() {
        res.push(vec[i]);
    }
    res
}

const ZIGZAG_INDICES_REV: [usize; 64] =
    [0, 1, 5, 6, 14, 15, 27, 28, 2, 4, 7, 13, 16, 26, 29, 42, 3, 8, 12, 17, 25, 30, 41, 43, 9, 11,
     18, 24, 31, 40, 44, 53, 10, 19, 23, 32, 39, 45, 52, 54, 20, 22, 33, 38, 46, 51, 55, 60, 21,
     34, 37, 47, 50, 56, 59, 61, 35, 36, 48, 49, 57, 58, 62, 63];

use std::fmt::Debug;
#[allow(dead_code)]
fn zigzag_inverse<I>(iter: I) -> Vec<I::Item>
    where I: Iterator,
          I::Item: Copy,
          I::Item: Default,
          I::Item: Debug
{
    let mut res: Vec<I::Item> = repeat(Default::default()).take(64).collect();
    for (zig_index, number) in iter.enumerate() {
        let original_index = ZIGZAG_INDICES[zig_index];
        res[original_index] = number;
    }
    res
}
