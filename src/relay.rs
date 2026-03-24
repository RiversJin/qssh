use anyhow::Result;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

async fn copy_direction<R, W>(
    mut reader: R,
    mut writer: W,
    buf_size: usize,
    direction: &'static str,
) -> Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; buf_size];
    let mut total: u64 = 0;

    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n]).await?;
        writer.flush().await?;
        total += n as u64;
        tracing::trace!(direction, bytes = n, "relayed chunk");
    }

    writer.shutdown().await.ok();
    tracing::debug!(direction, total, "relay direction finished");
    Ok(total)
}

pub async fn bidirectional<R1, W1, R2, W2>(
    r1: R1,
    w1: W1,
    r2: R2,
    w2: W2,
    buf_size: usize,
) -> Result<()>
where
    R1: AsyncRead + Unpin + Send + 'static,
    W1: AsyncWrite + Unpin + Send + 'static,
    R2: AsyncRead + Unpin + Send + 'static,
    W2: AsyncWrite + Unpin + Send + 'static,
{
    let a_to_b = tokio::spawn(copy_direction(r1, w2, buf_size, "local->remote"));
    let b_to_a = tokio::spawn(copy_direction(r2, w1, buf_size, "remote->local"));

    let (r1, r2) = tokio::join!(a_to_b, b_to_a);
    // Report the first error if any
    if let Err(e) = r1 {
        return Err(e.into());
    }
    if let Err(e) = r2 {
        return Err(e.into());
    }

    Ok(())
}
