// src/main.rs

mod gui;
mod oci_uploader;

use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::runtime::Runtime;
use uuid::Uuid;
use gui::RecordParams;
use pipewire::prelude::*;
use zbus::{Connection, ProxyBuilder};
use zbus::zvariant::Value;
use serde::Deserialize;
use libc;
use ffmpeg_next as ffmpeg;
use ffmpeg::format::io::IO;
use oci_uploader::OciUploader;

/// Структура для десериализации ответа метода Start портала.
#[derive(Debug, Deserialize)]
struct StartResponse {
    streams: Vec<StreamInfo>,
}

/// Информация о потоке (поле fd – файловый дескриптор).
#[derive(Debug, Deserialize)]
struct StreamInfo {
    fd: zbus::zvariant::Fd,
    node_id: u32,
}

/// Асинхронная функция, реализующая процесс захвата, кодирования и «записи» в OCI Object Storage.
async fn start_recording(params: RecordParams) -> Result<()> {
    println!("Starting screen recording with parameters: {:?}", params);

    // Формируем имя объекта: например, [filename_template].[container]
    let object_name = format!("{}.{}", params.filename_template, params.container);
    // Параметр output_folder здесь интерпретируется как имя OCI bucket.
    let bucket = params.output_folder; 

    // 1. Инициализируем Pipewire.
    pipewire::init();
    let _context = pipewire::Context::new()?;
    println!("Pipewire initialized.");

    // 2. Подключаемся к сеансовой шине D-Bus.
    let connection = Connection::session().await?;
    let proxy = ProxyBuilder::new_bare(&connection)
        .destination("org.freedesktop.portal.Desktop")?
        .path("/org/freedesktop/portal/desktop")?
        .interface("org.freedesktop.portal.ScreenCast")?
        .build()
        .await?;

    // 3. Создаём сессию с уникальным токеном.
    let session_token = Uuid::new_v4().to_string();
    let mut create_options: HashMap<&str, Value> = HashMap::new();
    create_options.insert("session_handle_token", Value::from(session_token));
    create_options.insert("types", Value::U32(3)); // захватываем экран и окна
    let (session_handle,): (String,) = proxy.call("CreateSession", &(create_options)).await?;
    println!("Session created: {}", session_handle);

    // 4. Вызываем SelectSources для выбора источников.
    let select_options: HashMap<&str, Value> = HashMap::new();
    let _ = proxy
        .call("SelectSources", &(session_handle.clone(), select_options))
        .await?;
    println!("SelectSources called.");

    // 5. Запускаем захват.
    let start_options: HashMap<&str, Value> = HashMap::new();
    let start_response: StartResponse = proxy
        .call("Start", &(session_handle.clone(), "rust_screen_recorder", start_options))
        .await?;
    println!("Start response: {:?}", start_response);

    let stream_info = start_response
        .streams
        .get(0)
        .ok_or_else(|| anyhow::anyhow!("No available streams in Start response"))?;
    println!("Using stream node_id: {}", stream_info.node_id);

    // Дублируем файловый дескриптор потока.
    let raw_fd = stream_info.fd.as_raw_fd();
    let dup_fd = unsafe { libc::dup(raw_fd) };
    if dup_fd < 0 {
        return Err(anyhow::anyhow!("Failed to duplicate file descriptor"));
    }
    println!("Duplicated FD: {}", dup_fd);

    // 6. Инициализируем FFmpeg.
    ffmpeg::init().map_err(|e| anyhow::anyhow!("FFmpeg init error: {:?}", e))?;
    let device_path = format!("/proc/self/fd/{}", dup_fd);
    println!("Opening input with ffmpeg: {}", device_path);

    let mut ictx = ffmpeg::format::input_with_format(&device_path, "pipewire")
        .map_err(|e| anyhow::anyhow!("Failed to open input stream: {:?}", e))?;

    let input_video_stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| anyhow::anyhow!("No video stream found in input"))?;
    let input_index = input_video_stream.index();
    println!("Input video stream index: {}", input_index);

    let mut decoder = input_video_stream
        .codec()
        .decoder()
        .video()
        .map_err(|e| anyhow::anyhow!("Failed to open video decoder: {:?}", e))?;

    // 7. Создаём объект-выгружатель (OciUploader) и оборачиваем его в Arc/Mutex.
    let uploader = Arc::new(Mutex::new(OciUploader::new(&bucket, &object_name)));
    // Создаём FFmpeg IO-контекст, который пишет в наш uploader.
    let io = IO::from_write(uploader.clone())
        .map_err(|e| anyhow::anyhow!("Failed to create FFmpeg IO: {:?}", e))?;
    // Создаём выходной формат с кастомным IO.
    let mut octx = ffmpeg::format::output_with_io(io)
        .map_err(|e| anyhow::anyhow!("Failed to create output context: {:?}", e))?;
    
    // 8. Настраиваем вывод: контейнер, кодек H264 и параметры из GUI.
    let global_header = octx.format().flags().contains(ffmpeg::format::flag::Flags::GLOBAL_HEADER);

    let codec = ffmpeg::encoder::find(ffmpeg::codec::Id::H264)
        .ok_or_else(|| anyhow::anyhow!("H264 encoder not found"))?;
    let mut ostream = octx.add_stream(codec)
        .map_err(|e| anyhow::anyhow!("Failed to add stream: {:?}", e))?;
    
    {
        let mut encoder = ostream
            .codec()
            .encoder()
            .video()
            .map_err(|e| anyhow::anyhow!("Failed to get video encoder: {:?}", e))?;
        encoder.set_width(decoder.width());
        encoder.set_height(decoder.height());
        encoder.set_format(ffmpeg::format::Pixel::YUV420P);
        encoder.set_time_base(decoder.time_base());
        encoder.set_bit_rate(params.bitrate as i64 * 1000); // битрейт в бит/с
        if global_header {
            encoder.set_flags(ffmpeg::codec::flag::Flags::GLOBAL_HEADER);
        }
        encoder.open_as(codec)
            .map_err(|e| anyhow::anyhow!("Failed to open video encoder: {:?}", e))?;
    }

    octx.write_header()
        .map_err(|e| anyhow::anyhow!("Failed to write header: {:?}", e))?;
    println!("Encoding started...");

    // 9. Обрабатываем пакеты: декодируем, кодируем и передаем в наш кастомный вывод (OCI uploader).
    for (stream, packet) in ictx.packets() {
        if stream.index() == input_index {
            decoder.send_packet(&packet)
                .map_err(|e| anyhow::anyhow!("Error sending packet to decoder: {:?}", e))?;
            loop {
                match decoder.receive_frame() {
                    Ok(mut frame) => {
                        let mut encoder = ostream
                            .codec()
                            .encoder()
                            .video()
                            .map_err(|e| anyhow::anyhow!("Error getting encoder: {:?}", e))?;
                        encoder.send_frame(&frame)
                            .map_err(|e| anyhow::anyhow!("Error sending frame to encoder: {:?}", e))?;
                        loop {
                            match encoder.receive_packet() {
                                Ok(mut encoded) => {
                                    encoded.set_stream(ostream.index());
                                    encoded.rescale_ts(decoder.time_base(), ostream.time_base());
                                    octx.write_packet(&encoded)
                                        .map_err(|e| anyhow::anyhow!("Error writing packet: {:?}", e))?;
                                },
                                Err(ffmpeg::Error::Other { .. })
                                | Err(ffmpeg::Error::Eof) => break,
                                Err(e) => return Err(anyhow::anyhow!("Error receiving encoded packet: {:?}", e)),
                            }
                        }
                    },
                    Err(ffmpeg::Error::Other { .. }) | Err(ffmpeg::Error::Eof) => break,
                    Err(e) => return Err(anyhow::anyhow!("Error receiving frame: {:?}", e)),
                }
            }
        }
    }

    decoder.send_eof()
        .map_err(|e| anyhow::anyhow!("Error sending EOF to decoder: {:?}", e))?;
    {
        let mut encoder = ostream
            .codec()
            .encoder()
            .video()
            .map_err(|e| anyhow::anyhow!("Error getting encoder for finishing: {:?}", e))?;
        encoder.send_eof()
            .map_err(|e| anyhow::anyhow!("Error sending EOF to encoder: {:?}", e))?;
        loop {
            match encoder.receive_packet() {
                Ok(mut encoded) => {
                    encoded.set_stream(ostream.index());
                    octx.write_packet(&encoded)
                        .map_err(|e| anyhow::anyhow!("Error writing final packet: {:?}", e))?;
                }
                Err(ffmpeg::Error::Other { .. })
                | Err(ffmpeg::Error::Eof) => break,
                Err(e) => return Err(anyhow::anyhow!("Error receiving final packet: {:?}", e)),
            }
        }
    }

    octx.write_trailer()
        .map_err(|e| anyhow::anyhow!("Error writing trailer: {:?}", e))?;
    println!("Encoding finished.");

    // После завершения записи вызываем finalize_upload, чтобы «отправить» данные в OCI.
    {
        let mut uploader = uploader.lock().unwrap();
        uploader.finalize_upload()
            .map_err(|e| anyhow::anyhow!("Error finalizing OCI upload: {:?}", e))?;
    }
    Ok(())
}

fn main() {
    gui::run_gui(move |params| {
        println!("GUI callback received parameters: {:?}", params);
        // Запускаем процесс записи в отдельном потоке с собственным tokio-рантаймом,
        // чтобы не блокировать GUI.
        thread::spawn(move || {
            let rt = Runtime::new().unwrap();
            if let Err(e) = rt.block_on(start_recording(params)) {
                eprintln!("Error during recording: {:?}", e);
            }
        });
    });
}
