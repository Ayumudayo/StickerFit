use super::{
    encode_native_png_bytes, inspect_input_media_internal, prepare_optimizer_plan,
    read_png_metadata_from_reader, timeline_duration_us, visit_decoded_animation_frame_iterator,
    write_native_apng, EditedTimelineFrame, OptimizerPlanRequest, ResolvedTimelineFrame,
    StickerFrame, DISCORD_MAX_DURATION_US, DISCORD_MAX_STICKER_BYTES,
};
use crate::estimation::OutputSizeEstimate;
use crate::locale::UiLocale;
use crate::media_error::PipelineError;
use crate::media_limits::MediaLimits;
use crate::operation::OperationContext;
use image::{Delay, DynamicImage, Frame, ImageFormat, Rgba, RgbaImage};
use serde_json::Value;
use std::fs::File;
use std::io::Cursor;
use std::time::Duration;

fn deterministic_pixels(width: u32, height: u32) -> RgbaImage {
    RgbaImage::from_fn(width, height, |x, y| {
        Rgba([
            (x.wrapping_mul(17) ^ y.wrapping_mul(29)) as u8,
            (x.wrapping_mul(7).wrapping_add(y.wrapping_mul(13))) as u8,
            (x.wrapping_mul(3) ^ y.wrapping_mul(5) ^ 0xa5) as u8,
            255,
        ])
    })
}

fn png_chunk(chunk_type: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(payload.len() + 12);
    bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    bytes.extend_from_slice(chunk_type);
    bytes.extend_from_slice(payload);
    let mut crc = crc32fast::Hasher::new();
    crc.update(chunk_type);
    crc.update(payload);
    bytes.extend_from_slice(&crc.finalize().to_be_bytes());
    bytes
}

fn parse_png_bytes(bytes: &[u8]) -> Result<(), PipelineError> {
    let mut reader = Cursor::new(bytes);
    read_png_metadata_from_reader(
        &mut reader,
        bytes.len() as u64,
        MediaLimits::default(),
        || Ok(()),
    )
    .map(|_| ())
}

fn plan_for_durations(durations_us: &[u64]) -> OptimizerPlanRequest {
    OptimizerPlanRequest {
        locale: Some("en".into()),
        source_duration_seconds: Some(
            durations_us.iter().copied().sum::<u64>() as f64 / 1_000_000.0,
        ),
        input_width: Some(48),
        input_height: Some(48),
        avg_fps: Some(durations_us.len() as f64),
        fit_mode: Some("contain".into()),
        preset_strategy: None,
        optimizer_goal: None,
        quality_frame_drop_interval: None,
        search_depth: None,
        crop_region: None,
        selected_frames: None,
        base_frame_count: Some(durations_us.len() as u32),
        timeline_frames: Some(
            durations_us
                .iter()
                .enumerate()
                .map(|(index, duration_us)| EditedTimelineFrame {
                    source_frame_id: index as u32 + 1,
                    duration_us: *duration_us,
                })
                .collect(),
        ),
    }
}

#[test]
fn generated_static_png_jpeg_and_bmp_are_inspectable() {
    let directory = tempfile::tempdir().expect("generated media directory must be created");
    let pixels = deterministic_pixels(16, 12);

    for (file_name, format, expected_format) in [
        ("generated.png", ImageFormat::Png, "png"),
        ("generated.jpg", ImageFormat::Jpeg, "jpg"),
        ("generated.bmp", ImageFormat::Bmp, "bmp"),
    ] {
        let path = directory.path().join(file_name);
        DynamicImage::ImageRgba8(pixels.clone())
            .save_with_format(&path, format)
            .expect("deterministic static fixture must encode");

        let inspection =
            inspect_input_media_internal(path.to_string_lossy().as_ref(), UiLocale::En);
        assert!(inspection.ok, "{expected_format} inspection must succeed");
        assert_eq!(inspection.format_name.as_deref(), Some(expected_format));
        assert_eq!((inspection.width, inspection.height), (Some(16), Some(12)));
        assert_eq!(inspection.tool_source.as_deref(), Some("native"));
        assert!(inspection.is_static_image);
    }
}

#[test]
fn generated_gif_and_apng_are_inspectable() {
    let directory = tempfile::tempdir().expect("generated media directory must be created");
    let first = deterministic_pixels(8, 6);
    let second = RgbaImage::from_pixel(8, 6, Rgba([9, 80, 170, 255]));

    let gif_path = directory.path().join("generated.gif");
    image::codecs::gif::GifEncoder::new(
        File::create(&gif_path).expect("generated GIF file must be created"),
    )
    .encode_frames(
        [
            Frame::from_parts(first.clone(), 0, 0, Delay::from_numer_denom_ms(100, 1)),
            Frame::from_parts(second.clone(), 0, 0, Delay::from_numer_denom_ms(200, 1)),
        ]
        .into_iter(),
    )
    .expect("deterministic GIF fixture must encode");

    let apng_path = directory.path().join("generated.png");
    write_native_apng(
        File::create(&apng_path).expect("generated APNG file must be created"),
        &[
            StickerFrame {
                pixels: first,
                duration_us: 100_000,
            },
            StickerFrame {
                pixels: second,
                duration_us: 200_000,
            },
        ],
        "standard",
    )
    .expect("deterministic APNG fixture must encode");

    for (path, expected_format) in [(gif_path, "gif"), (apng_path, "apng")] {
        let inspection =
            inspect_input_media_internal(path.to_string_lossy().as_ref(), UiLocale::En);
        assert!(inspection.ok, "{expected_format} inspection must succeed");
        assert_eq!(inspection.format_name.as_deref(), Some(expected_format));
        assert_eq!(inspection.estimated_frames, Some(2));
        assert_eq!((inspection.width, inspection.height), (Some(8), Some(6)));
        assert!(!inspection.is_static_image);
    }
}

#[test]
fn generated_frame_count_boundary_accepts_300_and_rejects_301() {
    let context = OperationContext::detached(Duration::from_secs(10));
    let frames = (0..300).map(|_| {
        Ok::<_, image::ImageError>(Frame::new(RgbaImage::from_pixel(
            1,
            1,
            Rgba([1, 2, 3, 255]),
        )))
    });
    let accepted = visit_decoded_animation_frame_iterator(
        "generated",
        frames,
        Some(300),
        MediaLimits::default(),
        &context,
        |_, _| Ok(()),
    )
    .expect("300 generated frames must be accepted");
    assert_eq!(accepted, 300);

    let frames = (0..301).map(|_| {
        Ok::<_, image::ImageError>(Frame::new(RgbaImage::from_pixel(
            1,
            1,
            Rgba([1, 2, 3, 255]),
        )))
    });
    let mut visited = 0;
    let error = visit_decoded_animation_frame_iterator(
        "generated",
        frames,
        Some(301),
        MediaLimits::default(),
        &context,
        |_, _| {
            visited += 1;
            Ok(())
        },
    )
    .expect_err("frame 301 must be rejected");
    assert_eq!(visited, 300);
    assert!(matches!(
        error,
        PipelineError::LimitExceeded {
            resource: "frame-count",
            limit: 300,
            actual: 301,
        }
    ));
}

#[test]
fn generated_duration_boundary_accepts_5000000_and_rejects_5000001_us() {
    let exact = [1_666_667, 1_666_667, 1_666_666];
    let timeline = exact
        .iter()
        .enumerate()
        .map(|(index, duration_us)| ResolvedTimelineFrame {
            source_frame_index: index as u32,
            duration_us: *duration_us,
        })
        .collect::<Vec<_>>();
    assert_eq!(timeline_duration_us(&timeline), Ok(DISCORD_MAX_DURATION_US));
    assert!(prepare_optimizer_plan(&plan_for_durations(&exact), UiLocale::En).ok);

    let over = prepare_optimizer_plan(
        &plan_for_durations(&[1_666_667, 1_666_667, 1_666_667]),
        UiLocale::En,
    );
    assert!(!over.ok);
    assert_eq!(over.reason_code.as_deref(), Some("duration-too-long"));
    assert!(over.candidates.is_empty());
}

#[test]
fn generated_output_size_boundary_matches_estimate_wire_limit() {
    let at_limit = serde_json::to_value(OutputSizeEstimate::exact_static(524_288))
        .expect("at-limit estimate must serialize");
    let over_limit = serde_json::to_value(OutputSizeEstimate::exact_static(524_289))
        .expect("over-limit estimate must serialize");

    assert_eq!(DISCORD_MAX_STICKER_BYTES, 524_288);
    assert_eq!(at_limit["limitBytes"].as_u64(), Some(524_288));
    assert_eq!(at_limit["bytes"].as_u64(), Some(524_288));
    assert!(at_limit["bytes"].as_u64() <= at_limit["limitBytes"].as_u64());
    assert!(over_limit["bytes"].as_u64() > over_limit["limitBytes"].as_u64());
}

#[test]
fn generated_malformed_png_forms_are_rejected_before_decode() {
    let invalid_signature = [0u8; 8];
    assert!(matches!(
        parse_png_bytes(&invalid_signature),
        Err(PipelineError::MalformedInput { format: "png", .. })
    ));

    let mut truncated = b"\x89PNG\r\n\x1a\n".to_vec();
    truncated.extend_from_slice(&13u32.to_be_bytes());
    assert!(matches!(
        parse_png_bytes(&truncated),
        Err(PipelineError::MalformedInput { format: "png", .. })
    ));

    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&1u32.to_be_bytes());
    ihdr[4..8].copy_from_slice(&1u32.to_be_bytes());
    ihdr[8] = 8;
    ihdr[9] = 6;
    let mut bad_crc = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = png_chunk(b"IHDR", &ihdr);
    *chunk.last_mut().expect("IHDR chunk has a CRC") ^= 0xff;
    bad_crc.extend_from_slice(&chunk);
    assert!(matches!(
        parse_png_bytes(&bad_crc),
        Err(PipelineError::MalformedInput { format: "png", reason })
            if reason.starts_with("CRC mismatch")
    ));

    let limits = MediaLimits::default();
    let mut oversized = b"\x89PNG\r\n\x1a\n".to_vec();
    oversized.extend_from_slice(&(limits.max_png_chunk_bytes + 1).to_be_bytes());
    oversized.extend_from_slice(b"tEXt");
    assert!(matches!(
        parse_png_bytes(&oversized),
        Err(PipelineError::LimitExceeded {
            resource: "png-chunk-bytes",
            limit,
            actual,
        }) if limit == u64::from(limits.max_png_chunk_bytes)
            && actual == u64::from(limits.max_png_chunk_bytes + 1)
    ));
}

#[test]
fn generated_high_entropy_320x320_png_is_deterministic() {
    let mut state = 0x4d59_5df4_d0f3_3173_u64;
    let mut raw = vec![0u8; 320 * 320 * 4];
    for pixel in raw.chunks_exact_mut(4) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        pixel[..3].copy_from_slice(&state.to_le_bytes()[..3]);
        pixel[3] = 255;
    }
    let pixels = RgbaImage::from_raw(320, 320, raw).expect("RGBA dimensions must be exact");

    let first = encode_native_png_bytes(&pixels).expect("high-entropy PNG must encode");
    let second = encode_native_png_bytes(&pixels).expect("repeat PNG encoding must succeed");
    assert_eq!(first, second);
    assert!(first.len() > 100_000, "fixture must remain high entropy");
    assert!((first.len() as u64) <= DISCORD_MAX_STICKER_BYTES);
    let decoded = image::load_from_memory_with_format(&first, ImageFormat::Png)
        .expect("generated PNG must decode")
        .to_rgba8();
    assert_eq!(decoded, pixels);
}

#[test]
fn checked_video_fixtures_match_the_source_contract_without_dynamic_decode() {
    const MANIFEST: &str = include_str!("../tests/fixtures/media-fixtures.json");
    const LICENSE: &str = include_str!("../tests/fixtures/LICENSE-chromium.txt");
    const MP4: &[u8] = include_bytes!("../tests/fixtures/tiny.mp4");
    const WEBM: &[u8] = include_bytes!("../tests/fixtures/tiny.webm");

    let manifest: Value = serde_json::from_str(MANIFEST).expect("fixture manifest must be valid");
    assert_eq!(manifest["schemaVersion"].as_u64(), Some(1));
    assert_eq!(
        manifest["upstream"]["commit"].as_str(),
        Some("da26bb3abb2ae5b7d97e91814c28efd20363c060")
    );
    assert_eq!(
        manifest["upstream"]["licenseFile"].as_str(),
        Some("LICENSE-chromium.txt")
    );
    assert_eq!(
        manifest["upstream"]["readmeUrl"].as_str(),
        Some(
            "https://chromium.googlesource.com/chromium/src/+/da26bb3abb2ae5b7d97e91814c28efd20363c060/media/test/data/README.md"
        )
    );
    assert_eq!(
        manifest["upstream"]["licenseSha256"].as_str(),
        Some("8c19aaf4ec1d6a59bb2c946461110cc5aac2bdc5b9d2d79336e46354fc9f1d8a")
    );
    assert_eq!(
        manifest["upstream"]["licenseSizeBytes"].as_u64(),
        Some(1_458)
    );
    assert_eq!(LICENSE.len(), 1_458);
    assert!(LICENSE.contains("Copyright 2015 The Chromium Authors"));

    let fixtures = manifest["fixtures"]
        .as_array()
        .expect("manifest fixtures must be an array");
    assert_eq!(fixtures.len(), 2);
    for (name, source_path, expected_size, bytes) in [
        (
            "tiny.mp4",
            "media/test/data/four-colors.mp4",
            12_141_u64,
            MP4,
        ),
        (
            "tiny.webm",
            "media/test/data/four-colors-vp9.webm",
            5_465_u64,
            WEBM,
        ),
    ] {
        let fixture = fixtures
            .iter()
            .find(|entry| entry["file"].as_str() == Some(name))
            .expect("every checked fixture must be declared");
        assert_eq!(fixture["sourcePath"].as_str(), Some(source_path));
        assert_eq!(fixture["sizeBytes"].as_u64(), Some(expected_size));
        assert_eq!(fixture["width"].as_u64(), Some(960));
        assert_eq!(fixture["height"].as_u64(), Some(540));
        assert_eq!(fixture["durationSeconds"].as_u64(), Some(2));
        assert!(fixture["sourceRecipe"]
            .as_str()
            .is_some_and(|recipe| !recipe.is_empty()));
        assert!(fixture["sourceCommand"].is_null());
        assert_eq!(bytes.len() as u64, expected_size);
    }
}

#[cfg(target_os = "windows")]
#[test]
fn checked_video_fixtures_are_inspectable_through_windows_runtime_paths() {
    let fixture_directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");

    let mp4 = inspect_input_media_internal(
        fixture_directory
            .join("tiny.mp4")
            .to_string_lossy()
            .as_ref(),
        UiLocale::En,
    );
    assert!(
        mp4.ok,
        "checked MP4 must be inspectable: {:?}",
        mp4.error_code
    );
    assert_eq!((mp4.width, mp4.height), (Some(960), Some(540)));
    assert!(mp4
        .duration_seconds
        .is_some_and(|duration| (duration - 2.0).abs() <= 0.3));
    match mp4.tool_source.as_deref() {
        Some("native") => {
            assert_eq!(mp4.format_name.as_deref(), Some("mp4"));
            assert!(mp4.tool_command.is_none());
            assert!(mp4.fallback_reason_code.is_none());
        }
        Some("sidecar") => {
            assert!(mp4
                .format_name
                .as_deref()
                .is_some_and(|format| format.contains("mp4")));
            assert!(mp4.tool_command.is_some());
            assert_eq!(
                mp4.fallback_reason_code.as_deref(),
                Some(super::MEDIA_FOUNDATION_FAILED_REASON_CODE)
            );
        }
        source => panic!("MP4 inspection used an unexpected tool source: {source:?}"),
    }

    let webm = inspect_input_media_internal(
        fixture_directory
            .join("tiny.webm")
            .to_string_lossy()
            .as_ref(),
        UiLocale::En,
    );
    assert!(
        webm.ok,
        "checked WebM must be inspectable: {:?}",
        webm.error_code
    );
    assert_eq!((webm.width, webm.height), (Some(960), Some(540)));
    assert!(webm
        .duration_seconds
        .is_some_and(|duration| (duration - 2.0).abs() <= 0.3));
    assert_eq!(webm.tool_source.as_deref(), Some("sidecar"));
    assert!(webm.tool_command.is_some());
    assert!(webm
        .format_name
        .as_deref()
        .is_some_and(|format| format.contains("webm")));
    assert!(webm
        .codec_name
        .as_deref()
        .is_some_and(|codec| codec.starts_with("vp9")));
}
