extern crate ffmpeg_next as ffmpeg;

use ffmpeg::format::{input, Pixel};
use ffmpeg::media::Type;
use ffmpeg::software::scaling::{context::Context, flag::Flags};
use ffmpeg::util::frame::video::Video;
use ffmpeg::codec::threading::Type as ThreadingType;
use std::env;
use std::{ptr, slice};
// use std::fs::File;
// use std::io::prelude::*;

use anyhow::Result;

use windows::{
    core::*,
    System::UserProfile::GlobalizationPreferences,
    Graphics::Imaging::{SoftwareBitmap, BitmapPixelFormat, BitmapBufferAccessMode},
    Media::Ocr::OcrEngine,
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

        let mut context_decoder = ffmpeg::codec::context::Context::from_parameters(input.parameters())?;
        let mut thread = context_decoder.threading();

        let process = 6;
        thread.count = process;
        thread.kind = ThreadingType::Frame;
        println!("thread: {}, type: {:?}", thread.count, thread.kind);

        context_decoder.set_threading(thread);
        // let  new_thread = context_decoder.threading();
        // println!("new thread: {}, type: {:?}", new_thread.count, new_thread.kind);

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
                let mut decoded = Video::empty();
                while decoder.receive_frame(&mut decoded).is_ok() {
                    let mut rgb_frame = Video::empty();
                    scaler.run(&decoded, &mut rgb_frame)?;
                    futures::executor::block_on(do_ocr(&rgb_frame, frame_index))?;
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
    }

    Ok(())
}

async fn do_ocr(frame: &Video, index: usize) -> std::result::Result<(), std::io::Error> {
    println!("{:?}", index);

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
            c[0] = if croped_rgb[i] >= 252 {
                croped_rgb[i]
            } else {
                20
            }
            // let r = croped_rgb[3 * i];
            // let g = croped_rgb[3 * i + 1];
            // let b = croped_rgb[3 * i + 2];

            // if r > 240 && g > 240 && b > 240 {
            //     c[0] = croped_rgb[3 * i];
            //     c[1] = croped_rgb[3 * i + 1];
            //     c[2] = croped_rgb[3 * i + 2];
            // } else {
            //     c[0] = 0;
            //     c[1] = 0;
            //     c[2] = 0;
            // }
            // c[3] = 255;
        });
    }
    // let zh_tw = OcrEngine::AvailableRecognizerLanguages().unwrap().GetAt(1).unwrap();

    // // let engine = OcrEngine::TryCreateFromUserProfileLanguages()?;
    // let engine = OcrEngine::TryCreateFromLanguage(&zh_tw)?;

    // // let lang2 = GlobalizationPreferences::Languages();
    // // println!("lang: {:?}", lang2);

    // let result = engine.RecognizeAsync(&bmp)?.await?;


    // println!("{:?}", result.Text()?.to_string());

    Ok(())
}
