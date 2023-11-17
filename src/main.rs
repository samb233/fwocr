extern crate ffmpeg_next as ffmpeg;

use ffmpeg::codec::threading::Type as ThreadingType;
use ffmpeg::format::{input, Pixel};
use ffmpeg::media::Type;
use ffmpeg::software::scaling::{context::Context, flag::Flags};
use ffmpeg::util::frame::video::Video;
use std::collections::HashMap;
use std::env;
use std::io::{BufRead, BufReader, Error, Write};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::{ptr, slice};
// use std::fs::File;
// use std::io::prelude::*;

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
    decode_video_and_ocr()
}

struct FrameMsg {
    index: usize,
    frame: Video,
}

struct SubMsg {
    index: usize,
    sub: String,
}

fn decode_video_and_ocr() -> Result<()> {
    ffmpeg::init().unwrap();

    if let Ok(mut ictx) = input(&env::args().nth(1).expect("Cannot open file.")) {
        let input = ictx
            .streams()
            .best(Type::Video)
            .ok_or(ffmpeg::Error::StreamNotFound)?;
        let video_stream_index = input.index();

        let frames = input.frames();
        println!("frame: {}", frames);

        let mut context_decoder =
            ffmpeg::codec::context::Context::from_parameters(input.parameters())?;
        let mut thread = context_decoder.threading();

        let process: usize = 6;
        thread.count = process;
        thread.kind = ThreadingType::Frame;
        println!("thread: {}, type: {:?}", thread.count, thread.kind);

        context_decoder.set_threading(thread);
        // let  new_thread = context_decoder.threading();
        // println!("new thread: {}, type: {:?}", new_thread.count, new_thread.kind);

        // let mut receiver_vec: Vec<Receiver<FrameMsg>> = Vec::new();
        let mut sender_vec: Vec<Sender<FrameMsg>> = Vec::new();

        let (sub_sender, sub_receiver) = mpsc::channel();

        for _ in 0..process {
            let (sender, receiver) = mpsc::channel();
            sender_vec.push(sender);

            let sub_sender_n = sub_sender.clone();

            thread::spawn(move || -> Result<()> {
                let zh_tw = OcrEngine::AvailableRecognizerLanguages()
                    .unwrap()
                    .GetAt(1)
                    .unwrap();

                // let engine = OcrEngine::TryCreateFromUserProfileLanguages()?;
                let engine = OcrEngine::TryCreateFromLanguage(&zh_tw)?;

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

        let res = thread::spawn(move || -> Vec<String> {
            // TODO:
            // 1. 文字后处理
            // 2. 存入到数组
            // 3. 当计数每一帧都处理之后，结束当前线程

            let mut v: Vec<String> = vec![String::from("");frames as usize + 1];
            let mut count = 1;

            for msg in sub_receiver {
                let sub = msg.sub.to_string();
                let sub_handled = after_handle(&sub);
                let s = String::from(sub_handled);

                // println!("index: {}, sub: {}", msg.index, sub_handled);
                v[msg.index] = s;

                count += 1;
                if count == frames {
                    break;
                }
            }

            v
        });

        let mut decoder = context_decoder.decoder().video()?;

        let mut scaler = Context::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            Pixel::GRAY8,
            960,
            540,
            // decoder.width() / 2,
            // decoder.height() / 2,
            Flags::BILINEAR,
        )?;

        let mut frame_index = 0;

        let mut receive_and_process_decoded_frames =
            |decoder: &mut ffmpeg::decoder::Video| -> Result<(), anyhow::Error> {
                let sender_index = frame_index % process;
                let sender = &sender_vec[sender_index];

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

        for (stream, packet) in ictx.packets() {
            if stream.index() == video_stream_index {
                decoder.send_packet(&packet)?;
                receive_and_process_decoded_frames(&mut decoder)?;
            }
        }
        decoder.send_eof()?;
        receive_and_process_decoded_frames(&mut decoder)?;

        let res_v = res.join().unwrap();

        let mut file = std::fs::File::create("test.txt")?;
        let mut frame_index = 1;
        for sub in &res_v {
            write!(file, "index: {}, sub: {}\n", frame_index, sub)?;
            frame_index += 1
        }
    }

    Ok(())
}

fn after_handle(s: &str) -> &str {
    let s_trim = s.trim();
    remove_not_chinese_left(s_trim)
}

async fn do_ocr(engine: &OcrEngine, frame: &Video) -> std::result::Result<String, std::io::Error> {
    // println!("{:?}", index);

    let rgb = frame.data(0);
    let width = 960;
    let height = 540;
    let croped_height = height / 5;
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
            c[0] = if croped_rgb[i] >= 225 {
                croped_rgb[i]
            } else {
                0
            }
        });
    }

    let result = engine.RecognizeAsync(&bmp)?.await?;
    Ok(result.Text()?.to_string())
}

fn remove_not_chinese_left(s: &str) -> &str {
    let mut not_chinese_index = 0;

    for c in s.chars() {
        if is_chinese_char(c) {
            break;
        }

        not_chinese_index += 1
    }

    if not_chinese_index == 0 {
        return s;
    } else {
        utf8_slice::from(s, not_chinese_index)
    }
}

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
