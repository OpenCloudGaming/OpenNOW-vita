// Adapted from green-vita (MPL-2.0, https://github.com/Day-OS/green-vita)

use super::{DecoderConfig, VideoPixelFormat, VideoTextureTarget};
#[cfg(not(target_os = "vita"))]
use anyhow::Result;

#[cfg(target_os = "vita")]
mod vita {
    use super::super::memory::{CdramBlock, release_reserved_decoder_cdram};
    use super::super::AU_PTS_STEP;
    use super::{DecoderConfig, VideoPixelFormat, VideoTextureTarget};
    use anyhow::{Result, bail};
    use std::os::raw::c_void;
    use vitasdk_sys::*;

    use super::super::AVCDEC_NUM_REF_FRAMES;

    const INTERNAL_CODEC_CONFIG: i32 = 2;
    const AVCDEC_MODE_EXTENDED: i32 = 0x80;
    const CODEC_MEMORY_ALIGNMENT: u32 = 1024 * 1024;
    const CODEC_VADDR_ALIGNMENT: u32 = 256 * 1024;

    #[link(name = "SceAvcodec_stub", kind = "static")]
    unsafe extern "C" {
        fn sceVideodecSetConfigInternal(codec_type: SceVideodecType, config: i32) -> i32;
        fn sceAvcdecSetDecodeMode(codec_type: SceVideodecType, mode: i32) -> i32;
        fn sceVideodecQueryMemSizeInternal(
            codec_type: SceVideodecType,
            query: *mut SceVideodecQueryInitInfo,
            size: *mut u32,
        ) -> i32;
        fn sceVideodecInitLibraryWithUnmapMemInternal(
            codec_type: SceVideodecType,
            control: *mut SceVideodecCtrl,
            query: *mut SceVideodecQueryInitInfo,
        ) -> i32;
        fn sceAvcdecQueryDecoderMemSizeInternal(
            codec_type: SceVideodecType,
            query: *mut SceAvcdecQueryDecoderInfo,
            decoder_info: *mut SceAvcdecDecoderInfo,
        ) -> i32;
        fn sceAvcdecCreateDecoderInternal(
            codec_type: SceVideodecType,
            decoder: *mut SceAvcdecCtrl,
            query: *mut SceAvcdecQueryDecoderInfo,
        ) -> i32;
        fn sceAvcdecDecodeAuInternal(
            decoder: *mut SceAvcdecCtrl,
            au: *mut SceAvcdecAu,
            picture_state: *mut i32,
        ) -> i32;
        fn sceAvcdecDecodeGetPictureWithWorkPictureInternal(
            decoder: *mut SceAvcdecCtrl,
            pictures: *mut SceAvcdecArrayPicture,
            work_pictures: *mut SceAvcdecArrayPicture,
            picture_state: *mut i32,
        ) -> i32;
    }

    #[link(name = "SceCodecEngine_stub", kind = "static")]
    unsafe extern "C" {
        fn sceCodecEngineOpenUnmapMemBlock(ptr: *mut c_void, size: u32) -> SceUID;
        fn sceCodecEngineCloseUnmapMemBlock(uid: SceUID) -> i32;
        fn sceCodecEngineAllocMemoryFromUnmapMemBlock(
            uid: SceUID,
            size: u32,
            alignment: u32,
        ) -> SceUIntVAddr;
        fn sceCodecEngineFreeMemoryFromUnmapMemBlock(uid: SceUID, address: SceUIntVAddr) -> i32;
    }

    #[repr(C)]
    struct SceVideodecCtrl {
        reserved: [u8; 24],
        vaddr: SceUIntVAddr,
        size: u32,
    }

    struct CodecEngineMemory {
        _block: CdramBlock,
        unmap_uid: SceUID,
        vaddr: SceUIntVAddr,
    }

    impl CodecEngineMemory {
        unsafe fn allocate(size: u32) -> Result<Self> {
            let block = CdramBlock::allocate_with_alignments(
                "opennow_avcdec_codec",
                size,
                CODEC_MEMORY_ALIGNMENT,
                CODEC_MEMORY_ALIGNMENT,
            )?;
            let block_size = block.capacity();
            let vaddr_size = size.div_ceil(CODEC_VADDR_ALIGNMENT) * CODEC_VADDR_ALIGNMENT;
            let unmap_uid = unsafe { sceCodecEngineOpenUnmapMemBlock(block.ptr.cast(), block_size) };
            if unmap_uid <= 0 {
                bail!("sceCodecEngineOpenUnmapMemBlock failed: {unmap_uid:#x}");
            }

            let vaddr = unsafe {
                sceCodecEngineAllocMemoryFromUnmapMemBlock(
                    unmap_uid,
                    vaddr_size,
                    CODEC_VADDR_ALIGNMENT,
                )
            };
            if vaddr == 0 {
                unsafe { sceCodecEngineCloseUnmapMemBlock(unmap_uid) };
                bail!("sceCodecEngineAllocMemoryFromUnmapMemBlock failed");
            }

            Ok(Self {
                _block: block,
                unmap_uid,
                vaddr,
            })
        }
    }

    impl Drop for CodecEngineMemory {
        fn drop(&mut self) {
            unsafe {
                sceCodecEngineFreeMemoryFromUnmapMemBlock(self.unmap_uid, self.vaddr);
                sceCodecEngineCloseUnmapMemBlock(self.unmap_uid);
            }
        }
    }

    struct AvcdecLibrary {
        module_loaded: bool,
        _codec_memory: CodecEngineMemory,
    }

    impl AvcdecLibrary {
        fn initialize(width: u32, height: u32) -> Result<Self> {
            let module_loaded = unsafe {
                let loaded_before = sceSysmoduleIsLoaded(SCE_SYSMODULE_AVCDEC);
                let ret = sceSysmoduleLoadModule(SCE_SYSMODULE_AVCDEC);
                if ret >= 0 {
                    true
                } else if ret as u32 == SCE_SYSMODULE_ERROR_INVALID_VALUE {
                    eprintln!(
                        "sceSysmoduleLoadModule(SCE_SYSMODULE_AVCDEC) returned {ret:#x}; continuing with SceVideodec imports; is_loaded_before={loaded_before:#x}",
                    );
                    false
                } else {
                    bail!(
                        "sceSysmoduleLoadModule(SCE_SYSMODULE_AVCDEC) failed: {ret:#x}; is_loaded_before={loaded_before:#x}",
                    );
                }
            };

            let mut init_info: SceVideodecQueryInitInfo = unsafe { std::mem::zeroed() };
            init_info.hwAvc = SceVideodecQueryInitInfoHwAvcdec {
                size: size_of::<SceVideodecQueryInitInfoHwAvcdec>() as u32,
                horizontal: width,
                vertical: height,
                numOfRefFrames: AVCDEC_NUM_REF_FRAMES,
                numOfStreams: 1,
            };

            let config_ret = unsafe {
                sceVideodecSetConfigInternal(SCE_VIDEODEC_TYPE_HW_AVCDEC, INTERNAL_CODEC_CONFIG)
            };
            if config_ret < 0 {
                bail!("sceVideodecSetConfigInternal failed: {config_ret:#x}");
            }
            let mode_ret =
                unsafe { sceAvcdecSetDecodeMode(SCE_VIDEODEC_TYPE_HW_AVCDEC, AVCDEC_MODE_EXTENDED) };
            if mode_ret < 0 {
                bail!("sceAvcdecSetDecodeMode failed: {mode_ret:#x}");
            }

            let mut codec_size = 0;
            let query_ret = unsafe {
                sceVideodecQueryMemSizeInternal(
                    SCE_VIDEODEC_TYPE_HW_AVCDEC,
                    &mut init_info,
                    &mut codec_size,
                )
            };
            if query_ret < 0 || codec_size == 0 {
                bail!(
                    "sceVideodecQueryMemSizeInternal failed: {query_ret:#x}, size={codec_size}"
                );
            }

            release_reserved_decoder_cdram();
            let codec_memory = unsafe { CodecEngineMemory::allocate(codec_size)? };
            let vaddr_size = codec_size.div_ceil(CODEC_VADDR_ALIGNMENT) * CODEC_VADDR_ALIGNMENT;
            let mut control = SceVideodecCtrl {
                reserved: [0; 24],
                vaddr: codec_memory.vaddr,
                size: vaddr_size,
            };

            let ret = unsafe {
                sceVideodecInitLibraryWithUnmapMemInternal(
                    SCE_VIDEODEC_TYPE_HW_AVCDEC,
                    &mut control,
                    &mut init_info,
                )
            };
            if ret < 0 {
                if module_loaded {
                    unsafe {
                        sceSysmoduleUnloadModule(SCE_SYSMODULE_AVCDEC);
                    }
                }
                bail!("sceVideodecInitLibraryWithUnmapMemInternal failed: {ret:#x}");
            }

            Ok(Self {
                module_loaded,
                _codec_memory: codec_memory,
            })
        }
    }

    impl Drop for AvcdecLibrary {
        fn drop(&mut self) {
            unsafe {
                sceVideodecTermLibrary(SCE_VIDEODEC_TYPE_HW_AVCDEC);
                if self.module_loaded {
                    sceSysmoduleUnloadModule(SCE_SYSMODULE_AVCDEC);
                }
            }
        }
    }

    struct AvcdecDecoder {
        ctrl: SceAvcdecCtrl,
    }

    impl Drop for AvcdecDecoder {
        fn drop(&mut self) {
            unsafe {
                sceAvcdecDeleteDecoder(&mut self.ctrl);
            }
        }
    }

    pub struct HwVideoDecoder {
        decoder: AvcdecDecoder,
        _frame_memory: CdramBlock,
        _library: AvcdecLibrary,
        width: u32,
        height: u32,
        decoder_timeout: i32,
        next_au_seq: u64,
    }

    impl HwVideoDecoder {
        pub fn new(config: DecoderConfig) -> Result<Self> {
            unsafe {
                let library =
                    AvcdecLibrary::initialize(config.decode_width, config.decode_height)?;

                let mut query = SceAvcdecQueryDecoderInfo {
                    horizontal: config.decode_width,
                    vertical: config.decode_height,
                    numOfRefFrames: AVCDEC_NUM_REF_FRAMES,
                };
                let mut decoder_info = SceAvcdecDecoderInfo { frameMemSize: 0 };
                let ret = sceAvcdecQueryDecoderMemSizeInternal(
                    SCE_VIDEODEC_TYPE_HW_AVCDEC,
                    &mut query,
                    &mut decoder_info,
                );
                if ret < 0 {
                    bail!("sceAvcdecQueryDecoderMemSizeInternal failed: {ret:#x}");
                }
                release_reserved_decoder_cdram();
                let frame_memory = CdramBlock::allocate_with_alignments(
                    "opennow_hw_video_frame",
                    decoder_info.frameMemSize,
                    CODEC_MEMORY_ALIGNMENT,
                    256 * 1024,
                )?;
                let mut decoder_control = SceAvcdecCtrl {
                    handle: 0,
                    frameBuf: SceAvcdecBuf {
                        pBuf: frame_memory.ptr.cast(),
                        size: decoder_info.frameMemSize,
                    },
                };
                let ret = sceAvcdecCreateDecoderInternal(
                    SCE_VIDEODEC_TYPE_HW_AVCDEC,
                    &mut decoder_control,
                    &mut query,
                );
                if ret < 0 {
                    bail!("sceAvcdecCreateDecoderInternal failed: {ret:#x}");
                }

                Ok(Self {
                    decoder: AvcdecDecoder {
                        ctrl: decoder_control,
                    },
                    _frame_memory: frame_memory,
                    _library: library,
                    width: config.output_width,
                    height: config.output_height,
                    decoder_timeout: 0,
                    next_au_seq: 0,
                })
            }
        }

        pub fn submitted_sequence(&self) -> u64 {
            self.next_au_seq
        }

        pub fn submit_access_unit(&mut self, access_unit: &[u8]) -> Result<()> {
            unsafe {
                self.next_au_seq = self.next_au_seq.saturating_add(1);
                let input_pts = self
                    .next_au_seq
                    .saturating_mul(AU_PTS_STEP);
                let mut au = SceAvcdecAu {
                    pts: SceVideodecTimeStamp {
                        upper: (input_pts >> 32) as u32,
                        lower: input_pts as u32,
                    },
                    dts: SceVideodecTimeStamp {
                        upper: (input_pts >> 32) as u32,
                        lower: input_pts as u32,
                    },
                    es: SceAvcdecBuf {
                        pBuf: access_unit.as_ptr() as *mut c_void,
                        size: access_unit.len() as u32,
                    },
                };
                let ret = sceAvcdecDecodeAuInternal(
                    &mut self.decoder.ctrl,
                    &mut au,
                    &mut self.decoder_timeout,
                );
                if ret < 0 {
                    bail!("sceAvcdecDecodeAuInternal failed: {ret:#x}");
                }
                Ok(())
            }
        }

        pub fn get_picture(
            &mut self,
            direct_target: VideoTextureTarget,
            format: VideoPixelFormat,
        ) -> Result<Option<u64>> {
            unsafe {
                let output_ptr = direct_target.ptr as *mut u8;
                let output_capacity = direct_target.capacity;
                let (pixel_type, output_pitch, required_capacity) = match format {
                    VideoPixelFormat::Bgr565 => (
                        SCE_AVCDEC_PIXELFORMAT_RGBA565 as u32,
                        direct_target.pitch / 2,
                        (direct_target.pitch / 2).saturating_mul(self.height) * 2,
                    ),
                    VideoPixelFormat::Iyuv => (
                        SCE_AVCDEC_PIXELFORMAT_YUV420_RASTER as u32,
                        direct_target.pitch,
                        self.width.saturating_mul(self.height) * 3 / 2,
                    ),
                    VideoPixelFormat::Rgba8888 => (
                        SCE_AVCDEC_PIXELFORMAT_RGBA8888 as u32,
                        direct_target.pitch / 4,
                        (direct_target.pitch / 4).saturating_mul(self.height) * 4,
                    ),
                };
                if output_pitch < self.width {
                    bail!(
                        "direct video texture pitch {output_pitch} is smaller than {}",
                        self.width
                    );
                }
                if required_capacity > output_capacity {
                    bail!(
                        "video output needs {required_capacity} bytes but texture has {output_capacity}"
                    );
                }

                let mut picture = SceAvcdecPicture {
                    size: size_of::<SceAvcdecPicture>() as u32,
                    frame: SceAvcdecFrame {
                        pixelType: pixel_type,
                        framePitch: output_pitch,
                        frameWidth: self.width,
                        frameHeight: self.height,
                        horizontalSize: self.width,
                        verticalSize: self.height,
                        frameCropLeftOffset: 0,
                        frameCropRightOffset: 0,
                        frameCropTopOffset: 0,
                        frameCropBottomOffset: 0,
                        opt: SceAvcdecFrameOption {
                            rgba: SceAvcdecFrameOptionRGBA {
                                alpha: 0xff,
                                cscCoefficient: 1, // ITU-R BT.709 for GFN HD video
                                reserved: [0; 14],
                            },
                        },
                        pPicture: match format {
                            VideoPixelFormat::Bgr565 | VideoPixelFormat::Rgba8888 => {
                                [output_ptr.cast(), std::ptr::null_mut()]
                            }
                            VideoPixelFormat::Iyuv => [
                                output_ptr.cast(),
                                output_ptr
                                    .add((self.width * self.height) as usize)
                                    .cast(),
                            ],
                        },
                    },
                    info: std::mem::zeroed(),
                };
                let mut picture_ptr: *mut SceAvcdecPicture = &mut picture;
                let mut array_picture = SceAvcdecArrayPicture {
                    numOfOutput: 0,
                    numOfElm: 1,
                    pPicture: &mut picture_ptr,
                };
                let mut work_picture = SceAvcdecArrayPicture {
                    numOfOutput: 0,
                    numOfElm: 0,
                    pPicture: std::ptr::null_mut(),
                };

                let ret = sceAvcdecDecodeGetPictureWithWorkPictureInternal(
                    &mut self.decoder.ctrl,
                    &mut array_picture,
                    &mut work_picture,
                    &mut self.decoder_timeout,
                );
                if ret < 0 {
                    bail!(
                        "sceAvcdecDecodeGetPictureWithWorkPictureInternal failed: {ret:#x}"
                    );
                }
                if array_picture.numOfOutput == 0 {
                    return Ok(None);
                }

                let returned_pts =
                    ((picture.info.pts.upper as u64) << 32) | picture.info.pts.lower as u64;
                Ok(Some(returned_pts))
            }
        }
    }

    unsafe impl Send for HwVideoDecoder {}
}

#[cfg(target_os = "vita")]
pub use vita::HwVideoDecoder;

#[cfg(not(target_os = "vita"))]
pub struct HwVideoDecoder;

#[cfg(not(target_os = "vita"))]
impl HwVideoDecoder {
    pub fn new(_config: DecoderConfig) -> Result<Self> {
        anyhow::bail!("hardware H.264 decoder is only available on the PS Vita target")
    }

    pub fn submitted_sequence(&self) -> u64 {
        0
    }

    pub fn submit_access_unit(&mut self, _access_unit: &[u8]) -> Result<()> {
        anyhow::bail!("hardware H.264 decoder is only available on the PS Vita target")
    }

    pub fn get_picture(
        &mut self,
        _direct_target: VideoTextureTarget,
        _format: VideoPixelFormat,
    ) -> Result<Option<u64>> {
        anyhow::bail!("hardware H.264 decoder is only available on the PS Vita target")
    }
}
