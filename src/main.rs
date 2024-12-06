extern crate ffmpeg_next as ffmpeg;

use ffmpeg::codec::threading::Type as ThreadingType;
use ffmpeg::format::{input, Pixel};
use ffmpeg::media::Type;
use ffmpeg::software::scaling::{context::Context, flag::Flags};
use ffmpeg::util::frame::video::Video;
use std::collections::HashMap;
use std::env;
use std::io::{BufRead, BufReader, Error, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::{ptr, slice};
// use std::fs::File;
// use std::io::prelude::*;

use indicatif::{MultiProgress, ProgressBar, ProgressState, ProgressStyle};

use anyhow::Result;

use windows::{
    core::*,
    Graphics::Imaging::{BitmapBufferAccessMode, BitmapPixelFormat, SoftwareBitmap},
    Media::Ocr::OcrEngine,
    System::UserProfile::GlobalizationPreferences,
    Win32::System::WinRT::*,
};

// use clap::{Parser};

// #[derive(Debug, Parser)]
// #[command(name = "fwocr")]
// #[command(about = "ffmpeg windows ocr", long_about = None)]
// struct Args {
//     #[arg(short, long)]
//     input: String,

//     #[arg(short, long)]
//     output: String,

//     #[arg(short, long)]
//     process: i8,

//     #[arg(short, long)]
//     lang: i8,
// }

fn main() -> Result<()> {
    let filename = env::args().nth(1).expect("Cannot open file.");
    let thread_count = 6;
    let lang = 2;

    let mb = MultiProgress::new();
    let decode_sty = ProgressStyle::with_template(
        "Decode:{pos:>6}/{len:6} [{elapsed_precise}] (eta: {eta}) [{bar:40.cyan/blue}]",
    )
    .unwrap()
    .progress_chars("##-");

    let decode_pb = mb.add(ProgressBar::new(35246));
    decode_pb.set_style(decode_sty.clone());

    let ocr_sty = ProgressStyle::with_template(
        "OCR:   {pos:>6}/{len:6} [{elapsed_precise}] (eta: {eta}) [{bar:40.cyan/blue}]",
    )
    .unwrap()
    .progress_chars("##-");
    let ocr_pb = mb.add(ProgressBar::new(35246));
    ocr_pb.set_style(ocr_sty.clone());

    let mut frame_senders: Vec<Sender<FrameMsg>> = Vec::new();
    let mut frame_receivers: Vec<Receiver<FrameMsg>> = Vec::new();
    for _ in 0..thread_count {
        let (frame_sender, frame_receiver) = mpsc::channel();
        frame_senders.push(frame_sender);
        frame_receivers.push(frame_receiver);
    }

    let decode_conf = DecoderConfig {
        filename,
        dst_format: Pixel::GRAY8,
        dst_w: 960,
        dst_h: 540,
        thread_count,
        senders: frame_senders,
    };

    let (sub_sender, sub_receiver) = mpsc::channel();
    let ocr_conf = OcrConfig {
        lang,
        frame_receivers,
        sub_sender,
    };

    thread::spawn(move || -> Result<()> { decode_video_and_ocr(decode_conf, decode_pb) });

    thread::spawn(move || -> Result<()> { ocr(ocr_conf) });

    let handle = thread::spawn(move || -> Result<()> { handle(sub_receiver, 35246, ocr_pb) });

    handle.join().unwrap()?;
    mb.clear().unwrap();
    Ok(())
}

struct FrameMsg {
    index: usize,
    frame: Video,
}

struct SubMsg {
    index: usize,
    sub: String,
}

struct Subtitle {
    start_frame: usize,
    end_frame: usize,
    text: String,
}

struct DecoderConfig {
    filename: String,
    dst_format: Pixel,
    dst_w: u32,
    dst_h: u32,

    thread_count: usize,
    senders: Vec<Sender<FrameMsg>>,
}

struct OcrConfig {
    lang: u32,

    frame_receivers: Vec<Receiver<FrameMsg>>,
    sub_sender: Sender<SubMsg>,
}

fn decode_video_and_ocr(conf: DecoderConfig, pb: ProgressBar) -> Result<()> {
    ffmpeg::init().unwrap();

    let mut ictx = input(&conf.filename)?;

    let input = ictx
        .streams()
        .best(Type::Video)
        .ok_or(ffmpeg::Error::StreamNotFound)?;
    let video_stream_index = input.index();

    let frames = input.frames();
    pb.set_length(frames as u64);
    // println!("frame: {}", frames);

    let mut context_decoder = ffmpeg::codec::context::Context::from_parameters(input.parameters())?;
    let mut thread = context_decoder.threading();

    let process: usize = conf.thread_count;
    thread.count = process;
    thread.kind = ThreadingType::Frame;
    // println!("thread: {}, type: {:?}", thread.count, thread.kind);

    context_decoder.set_threading(thread);
    // let  new_thread = context_decoder.threading();
    // println!("new thread: {}, type: {:?}", new_thread.count, new_thread.kind);

    // let mut receiver_vec: Vec<Receiver<FrameMsg>> = Vec::new();

    let mut decoder = context_decoder.decoder().video()?;

    let mut scaler = Context::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        conf.dst_format,
        conf.dst_w,
        conf.dst_h,
        Flags::SPLINE,
    )?;

    let mut frame_index = 0;

    let mut receive_and_process_decoded_frames =
        |decoder: &mut ffmpeg::decoder::Video| -> Result<(), anyhow::Error> {
            let sender_index = frame_index % process;
            let sender = &conf.senders[sender_index];

            let mut decoded = Video::empty();
            while decoder.receive_frame(&mut decoded).is_ok() {
                let mut rgb_frame = Video::empty();
                scaler.run(&decoded, &mut rgb_frame)?;

                let msg = FrameMsg {
                    index: frame_index,
                    frame: rgb_frame,
                };
                sender.send(msg).unwrap();

                // futures::executor::block_on(do_ocr(&rgb_frame, frame_index))?;
                // save_file(&rgb_frame, frame_index).unwrap();
                frame_index += 1;
            }
            Ok(())
        };

    let start_frame = 0;
    let mut frame_index = 0;
    // let mut is_seeking_key = true;

    for (stream, packet) in ictx.packets() {
        if stream.index() == video_stream_index {
            if frame_index < start_frame {
                frame_index += 1;
                continue;
            }

            // if is_seeking_key {
            //     if !packet.is_key() {
            //         frame_index += 1;
            //         continue;
            //     } else {
            //         is_seeking_key = false;
            //         println!("start_frame: {}", frame_index)
            //     }
            // }

            decoder.send_packet(&packet)?;
            receive_and_process_decoded_frames(&mut decoder)?;
            pb.inc(1);
        }
    }
    decoder.send_eof()?;
    receive_and_process_decoded_frames(&mut decoder)?;

    pb.inc(1);
    pb.finish();

    Ok(())
}

fn ocr(conf: OcrConfig) -> Result<()> {
    for receiver in conf.frame_receivers {
        let sub_sender_n = conf.sub_sender.clone();

        thread::spawn(move || -> Result<()> {
            let lang = OcrEngine::AvailableRecognizerLanguages()
                .unwrap()
                .GetAt(conf.lang)
                .unwrap();

            // let engine = OcrEngine::TryCreateFromUserProfileLanguages()?;
            let engine = OcrEngine::TryCreateFromLanguage(&lang)?;

            // let lang2 = GlobalizationPreferences::Languages();
            // println!("lang: {:?}", lang2);

            for msg in receiver {
                let result = futures::executor::block_on(do_ocr(&engine, &msg.frame))?;
                let sub_msg = SubMsg {
                    index: msg.index,
                    sub: result,
                };

                sub_sender_n.send(sub_msg).unwrap();
            }

            Ok(())
        });
    }

    Ok(())
}

fn handle(sub_receiver: Receiver<SubMsg>, total: usize, pb: ProgressBar) -> Result<()> {
    let mut v: Vec<String> = vec![String::from(""); total + 1];
    let mut count = 1;
    for msg in sub_receiver {
        let sub = msg.sub.to_string();
        let sub_handled = after_handle(&sub);
        let s = String::from(sub_handled);

        v[msg.index] = s;
        count += 1;
        pb.inc(1);

        if count == total {
            break;
        }
    }
    pb.finish();

    let mut subtitle_vec: Vec<Subtitle> = Vec::new();

    let mut frame_index = 0;
    while frame_index < total {
        let sub = v.get(frame_index).unwrap();

        if sub.len() == 0 {
            frame_index += 1;
            continue;
        }

        let mut end_index = frame_index;

        let mut sub_map: HashMap<String, i32> = HashMap::new();
        let count = sub_map.entry(sub.clone()).or_insert(0);
        *count += 1;

        for end in frame_index + 1..total {
            let next_sub = v.get(end as usize).unwrap();

            if next_sub.len() == 0 {
                end_index = end - 1;
                break;
            }

            let mut both_count = 0;
            for diff in diff::chars(sub, next_sub) {
                match diff {
                    diff::Result::Both(_, _) => both_count += 1,
                    _ => continue,
                }
            }

            if both_count == 0 {
                end_index = end - 1;
                break;
            }

            let count = sub_map.entry(next_sub.clone()).or_insert(0);
            *count += 1;
        }

        if sub_map.len() == 0 {
            frame_index += 1;
            continue;
        }

        let mut max_count = 0;
        let mut current_sub: String = String::new();
        for (sub, count) in sub_map.iter() {
            if *count > max_count {
                max_count = *count;
                current_sub = sub.clone();
            }
        }

        let mut subtitle = Subtitle {
            start_frame: frame_index,
            end_frame: end_index,
            text: current_sub,
        };

        if subtitle_vec.len() > 0 {
            let sub_last = subtitle_vec.pop().unwrap();
            if sub.clone() == sub_last.text {
                subtitle.start_frame = sub_last.start_frame;
                subtitle_vec.push(subtitle);
            } else {
                subtitle_vec.push(sub_last);
                subtitle_vec.push(subtitle);
            }
        } else {
            subtitle_vec.push(subtitle);
        }

        frame_index = end_index + 1;
    }

    let mut file = std::fs::File::create("test.txt")?;
    for subtitle in subtitle_vec {
        write!(
            file,
            "start: {}, end:{}, sub: {}\n",
            subtitle.start_frame, subtitle.end_frame, subtitle.text
        )?;
    }

    Ok(())
}

fn after_handle(s: &str) -> String {
    s.replace(" ", "")
    // remove_not_chinese_left(s_trim)
}

async fn do_ocr(engine: &OcrEngine, frame: &Video) -> std::result::Result<String, std::io::Error> {
    // println!("{:?}", index);

    let rgb = frame.data(0);
    let width = 960;
    let height = 540;
    let croped_height = height / 6;
    let croped_pixels = width * (height - croped_height);
    let croped_rgb = &rgb[croped_pixels as usize..];

    // 将帧数据的u8数组写入到SoftwareBitmap的魔法
    // 来源：https://qiita.com/benki/items/c22985e1fa7d1ffc4caf
    let bmp = SoftwareBitmap::Create(BitmapPixelFormat::Gray8, width, croped_height)?;
    {
        let bmp_buf = bmp.LockBuffer(BitmapBufferAccessMode::Write)?;
        let array: IMemoryBufferByteAccess = bmp_buf.CreateReference()?.cast()?;

        let mut data = ptr::null_mut();
        let mut capacity = 0;
        unsafe {
            array.GetBuffer(&mut data, &mut capacity)?;
        }
        assert_eq!((width * croped_height).abs(), capacity as i32);

        let slice = unsafe { slice::from_raw_parts_mut(data, capacity as usize) };
        slice.chunks_mut(1).enumerate().for_each(|(i, c)| {
            // c[0] = croped_rgb[i]
            c[0] = if croped_rgb[i] > 220 {
                croped_rgb[i]
            } else {
                0
            }
        });
    }

    let result = engine.RecognizeAsync(&bmp)?.await?;
    Ok(result.Text()?.to_string())
}

// fn remove_not_chinese_left(s: &str) -> &str {
//     let mut not_chinese_index = 0;

//     for c in s.chars() {
//         if is_chinese_char(c) {
//             break;
//         }

//         not_chinese_index += 1
//     }

//     if not_chinese_index == 0 {
//         return s;
//     } else {
//         utf8_slice::from(s, not_chinese_index)
//     }
// }

fn is_chinese_char(ch: char) -> bool {
    match ch as u32 {
        0x4e00..=0x9fff => true,
        // 0xff0c => false,           //，
        // 0x3002 => false,           //。
        // 0x3400..=0x4dbf => true,   // CJK Unified Ideographs Extension A
        // 0x20000..=0x2a6df => true, // CJK Unified Ideographs Extension B
        // 0x2a700..=0x2b73f => true, // CJK Unified Ideographs Extension C
        // 0x2b740..=0x2b81f => true, // CJK Unified Ideographs Extension D
        // 0x2b820..=0x2ceaf => true, // CJK Unified Ideographs Extension E
        // 0x3300..=0x33ff => true,   // https://en.wikipedia.org/wiki/CJK_Compatibility
        // 0xfe30..=0xfe4f => true,   // https://en.wikipedia.org/wiki/CJK_Compatibility_Forms
        // 0xf900..=0xfaff => true,   // https://en.wikipedia.org/wiki/CJK_Compatibility_Ideographs
        // 0x2f800..=0x2fa1f => true, // https://en.wikipedia.org/wiki/CJK_Compatibility_Ideographs_Supplement
        // 0x00b7 => false,           //·
        // 0x00d7 => false,           //×
        // 0x2014 => false,           //—
        // 0x2018 => false,           //‘
        // 0x2019 => false,           //’
        // 0x201c => false,           //“
        // 0x201d => false,           //”
        // 0x2026 => false,           //…
        // 0x3001 => false,           //、
        // 0x300a => false,           //《
        // 0x300b => false,           //》
        // 0x300e => false,           //『
        // 0x300f => false,           //』
        // 0x3010 => false,           //【
        // 0x3011 => false,           //】
        // 0xff01 => false,           //！
        // 0xff08 => false,           //（
        // 0xff09 => false,           //）
        // 0xff1a => false,           //：
        // 0xff1f => false,           //？
        _ => false,
    }
}
