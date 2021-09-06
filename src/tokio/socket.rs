// Copyright (C) 2020 - Will Glozer. All rights reserved.

use crate::option::{Level, Name, Opt};
use crate::{Domain, Protocol, Type};
use futures::ready;
use std::io::{self, IoSlice, IoSliceMut, Result};
use std::net::{SocketAddr, ToSocketAddrs};
use std::os::unix::prelude::*;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite};

pub struct RawSocket {
    io: AsyncFd<crate::RawSocket>,
}

impl RawSocket {
    pub fn new(domain: Domain, kind: Type, protocol: Option<Protocol>) -> Result<Self> {
        let sys = crate::RawSocket::new(domain, kind, protocol)?;
        sys.set_nonblocking(true)?;
        let io = AsyncFd::new(sys)?;
        Ok(RawSocket { io })
    }

    pub async fn bind<A: ToSocketAddrs>(&self, addr: A) -> Result<()> {
        self.io.get_ref().bind(addr)
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.io.get_ref().local_addr()
    }

    pub async fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr)> {
        self.read(|s| s.recv_from(buf)).await
    }

    pub async fn recv_msg(
        &self,
        data: &[IoSliceMut<'_>],
        ctrl: Option<&mut [u8]>,
    ) -> Result<(usize, SocketAddr)> {
        let ctrl = ctrl.unwrap_or(&mut []);
        self.read(|s| s.recv_msg(data, ctrl)).await
    }

    pub async fn send_to<A: ToSocketAddrs>(&self, buf: &[u8], addr: A) -> Result<usize> {
        self.write(|s| s.send_to(buf, &addr)).await
    }

    pub async fn send_msg<A: ToSocketAddrs>(
        &self,
        addr: A,
        data: &[IoSlice<'_>],
        ctrl: Option<&[u8]>,
    ) -> Result<usize> {
        let ctrl = ctrl.unwrap_or(&[]);
        self.write(|s| s.send_msg(&addr, data, ctrl)).await
    }

    pub fn get_sockopt<O: Opt>(&self, level: Level, name: Name) -> Result<O> {
        self.io.get_ref().get_sockopt(level, name)
    }

    pub fn set_sockopt<O: Opt>(&self, level: Level, name: Name, value: &O) -> Result<()> {
        self.io.get_ref().set_sockopt(level, name, value)
    }

    async fn read<F: FnMut(&crate::RawSocket) -> Result<R>, R>(&self, mut f: F) -> Result<R> {
        loop {
            let mut guard = self.io.readable().await?;
            match guard.try_io(|inner| f(inner.get_ref())) {
                Ok(r) => return r,
                Err(_) => continue,
            }
        }
    }

    async fn write<F: FnMut(&crate::RawSocket) -> Result<R>, R>(&self, mut f: F) -> Result<R> {
        loop {
            let mut guard = self.io.writable().await?;
            match guard.try_io(|inner| f(inner.get_ref())) {
                Ok(r) => return r,
                Err(_) => continue,
            }
        }
    }
}

impl AsyncWrite for RawSocket {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = ready!(self.io.poll_write_ready_mut(cx))?;

            match guard.try_io(|inner| inner.get_mut().write(buf)) {
                Ok(result) => return Poll::Ready(result),
                Err(_would_block) => continue,
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.io.get_ref().shutdown(std::net::Shutdown::Write)?;
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for RawSocket {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let mut guard = ready!(self.io.poll_read_ready_mut(cx))?;

            match guard.try_io(|inner| {
                let b = unsafe { buf.unfilled_mut() };
                let b = unsafe { std::mem::transmute::<_, &mut [u8]>(b) };

                inner.get_mut().read(b)
            }) {
                Ok(result) => {
                    let result = result.and_then(|size| {
                        unsafe { buf.assume_init(size) };
                        buf.advance(size);
                        Ok(())
                    });
                    return Poll::Ready(result);
                }
                Err(_would_block) => continue,
            }
        }
    }
}

impl AsRawFd for RawSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.io.get_ref().as_raw_fd()
    }
}
