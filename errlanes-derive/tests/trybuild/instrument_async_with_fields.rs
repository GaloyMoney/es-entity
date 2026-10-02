#[derive(Debug, errlanes::Rejection)]
enum MyRejection {
    Closed,
}

struct Job;

impl Job {
    #[errlanes::instrument(name = "job.fail_job", skip(self), fields(job_id = tracing::field::Empty, attempt))]
    async fn fail_job(
        &mut self,
        id: u64,
        attempt: u32,
    ) -> Result<(), errlanes::Fail<MyRejection>> {
        let _ = (id, attempt);
        Err(MyRejection::Closed.into())
    }
}

fn main() {
    let _ = Job::fail_job;
}
