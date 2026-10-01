// rustfmt wraps a long `#[errlanes::instrument(..)]` argument list (or a long
// `fields(..)` group) onto multiple lines with a trailing comma. Appending
// another comma unconditionally before the injected `FIELDS` entries would
// expand to `,,` and fail to compile — this fixture is that exact shape.
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum MyRejection {
    #[error("closed")]
    Closed,
}

struct Job;

impl Job {
    #[errlanes::instrument(
        name = "job.fail_job",
        skip(self),
        fields(job_id = tracing::field::Empty,),
    )]
    async fn fail_job(
        &mut self,
        id: u64,
    ) -> Result<(), errlanes::Fail<MyRejection>> {
        let _ = id;
        Err(MyRejection::Closed.into())
    }
}

fn main() {
    let _ = Job::fail_job;
}
