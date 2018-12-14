extern crate imap;
extern crate md5;
extern crate native_tls;
extern crate regex;

use imap::error::Result;
use std::io;
use std::io::Write;
use std::net;
use std::ops;
use std::str;

const PAGE: u32 = 10;

struct ImapSession {
    session: imap::Session<native_tls::TlsStream<net::TcpStream>>,
}

impl ImapSession {
    fn new(domain: &str, user: &str, password: &str) -> Result<ImapSession> {
        let tls = native_tls::TlsConnector::builder().build()?;
        let client = imap::connect((domain, 993), domain, &tls)?;
        let session = client.login(user, password).map_err(|e| e.0)?;

        Ok(ImapSession { session })
    }
}

impl Drop for ImapSession {
    fn drop(&mut self) {
        #[allow(unused_must_use)]
        self.session.logout();
    }
}

impl ops::Deref for ImapSession {
    type Target = imap::Session<native_tls::TlsStream<net::TcpStream>>;

    fn deref(&self) -> &Self::Target {
        &self.session
    }
}

impl ops::DerefMut for ImapSession {
    fn deref_mut(&mut self) -> &mut imap::Session<native_tls::TlsStream<net::TcpStream>> {
        &mut self.session
    }
}

fn collect_uids(session: &mut ImapSession, mailbox: &str) -> Result<Vec<u32>> {
    let mb = session.select(mailbox)?;
    let fetch = session.fetch(format!("1:{}", mb.exists), "(UID)")?;

    Ok(fetch.iter().map(|x| x.uid.unwrap()).collect())
}

fn clear(session: &mut ImapSession, mailbox: &str) -> Result<()> {
    let mb = session.select(mailbox)?;
    if mb.exists > 0 {
        session.store(format!("1:{}", mb.exists), "+FLAGS (\\Deleted)")?;
    }
    session.expunge()?;

    Ok(())
}

fn copy_emails(
    src: &mut ImapSession,
    dst: &mut ImapSession,
    src_mailbox: &str,
    dst_mailbox: &str,
    flags: &str,
    mut ignore_uids: Vec<u32>,
) -> Result<()> {
    ignore_uids.sort();

    let mb = src.select(src_mailbox)?;
    let email_count = mb.exists;
    println!("Email count on {}: {}", src_mailbox, email_count);

    let mut i = 1;
    loop {
        let end = i + PAGE - 1;
        let messages = src.fetch(format!("{}:{}", i, end), "(RFC822 FLAGS UID)")?;
        for x in messages
            .iter()
            .filter(|x| ignore_uids.binary_search(&x.uid.unwrap()).is_err())
        {
            dst.append(dst_mailbox, x.body().unwrap())?;
            print!(
                "{}/{} ({:.02}%) \r",
                i,
                email_count,
                (i as f64 / email_count as f64 * 100.0)
            );
            io::stdout().flush().unwrap();
            i += 1;
        }

        i = end + 1;
        if end > email_count {
            break;
        }
    }
    println!();

    let mb = dst.select(dst_mailbox)?;
    dst.store(format!("1:{}", mb.exists), format!("+FLAGS ({})", flags))?;

    Ok(())
}

fn delete_duplicates(session: &mut ImapSession, mailbox: &str) -> Result<()> {
    use md5::{Digest, Md5};
    use std::collections::HashSet;

    let mb = session.select(mailbox)?;

    let mut candidates = Vec::new();
    let re = regex::Regex::new(r"(?i)(?m)(Date|Subject):\s+(.+)").unwrap();
    let messages = session.fetch(format!("1:{}", mb.exists), "(RFC822.HEADER UID)")?;
    let mut prev_key = "".to_string();
    let mut prev_uid = 0;
    for x in messages.iter() {
        let uid = x.uid.unwrap();
        let s = str::from_utf8(x.header().unwrap()).unwrap();
        let key = re
            .captures_iter(s)
            .map(|x| x.get(0).unwrap().as_str().chars())
            .flatten()
            .collect();
        if key == prev_key {
            candidates.push(prev_uid);
            candidates.push(uid);
        }

        prev_uid = uid;
        prev_key = key;
    }
    println!("{:?}", candidates);

    let mut hashes = HashSet::new();
    for uid in candidates {
        let messages = session.uid_fetch(format!("{}", uid), "(RFC822)")?;
        let x = messages.iter().next().unwrap();
        let hash = Md5::digest(x.body().unwrap());
        if hashes.contains(&hash) {
            println!("{} {:x}", x.message, hash);
            session.uid_store(format!("{}", uid), "+FLAGS (\\Deleted)")?;
        } else {
            hashes.insert(hash);
        }
    }
    session.expunge()?;

    Ok(())
}

fn delete_sent(session: &mut ImapSession, mailbox: &str) -> Result<()> {
    let mb = session.select(mailbox)?;
    let re = regex::Regex::new(r"(?i)(?m)From:\s+(.*cecile.tonglet@gmail.com.*)").unwrap();
    let messages = session.fetch(format!("1:{}", mb.exists), "(RFC822.HEADER UID)")?;
    let mut uids = Vec::new();
    for x in messages.iter() {
        let uid = x.uid.unwrap();
        let s = str::from_utf8(x.header().unwrap()).unwrap();
        let key: String = re
            .captures_iter(s)
            .map(|x| x.get(0).unwrap().as_str().chars())
            .flatten()
            .collect();
        if key != "" {
            println!("{} {}", uid, key);
            uids.push(uid);
        }
    }
    if uids.len() > 0 {
        session.uid_store(
            uids.iter()
                .map(|x| format!("{}", x).to_owned())
                .collect::<Vec<_>>()
                .join(","),
            "+FLAGS (\\Deleted)",
        )?;
        session.expunge()?;
        println!("{} deleted.", uids.len());
    }
    Ok(())
}

fn search(session: &mut ImapSession, mailbox: &str, astring: &str) -> Result<()> {
    let mb = session.select(mailbox)?;
    let ids = session.search(format!("BODY \"{}\"", astring));
    println!("{:?}", ids);
    Ok(())
}

fn run() -> Result<()> {
    /*
    let mut ignore_uids = collect_uids(&mut session1, "[Gmail]/Drafts")?;
    ignore_uids.append(&mut collect_uids(&mut session1, "[Gmail]/Sent Mail")?);

    clear(&mut session2, "Sent")?;
    copy_emails(&mut session1, &mut session2, "[Gmail]/Sent Mail", "Sent", "\\Seen", vec![])?;

    clear(&mut session2, "Drafts")?;
    copy_emails(&mut session1, &mut session2, "[Gmail]/Drafts", "Drafts", "\\Seen \\Draft", vec![])?;

    clear(&mut session2, "INBOX")?;
    copy_emails(&mut session1, &mut session2, "[Gmail]/All Mail", "INBOX", "\\Seen", ignore_uids)?;
    */
    //delete_duplicates(&mut session2, "INBOX")?;
    //delete_sent(&mut session2, "INBOX")?;

    Ok(())
}

fn main() {
    match run() {
        Err(err) => eprintln!("{}", err),
        Ok(()) => println!("end."),
    }
}
