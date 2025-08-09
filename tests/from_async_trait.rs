mod entities;
mod helpers;

use es_entity::*;
use helpers::init_pool;
use sqlx::PgPool;

use entities::order::*;

// This test is mainly here to check if the library compiles and can be used from within
// async_trait fns.
// When initially writing the AtomicOperation / OneTimeExecutor stuff it was very painful
// to find a combination of generics that made the compiler happy and were ergonomic to use.

#[async_trait::async_trait]
trait RunJob {
    async fn execute(&self) -> anyhow::Result<()>;
}

struct TestJob {
    pool: PgPool,
}

#[async_trait::async_trait]
impl RunJob for TestJob {
    async fn execute(&self) -> anyhow::Result<()> {
        let pool = self.pool.clone();

        let orders = Orders::new(pool);
        let order_id = OrderId::new();
        let new_order = NewOrderBuilder::default().id(order_id).build().unwrap();

        let _ = orders.create(new_order).await?;
        let mut op = orders.begin_op().await?;
        let mut order = orders.find_by_id_in_op(&mut op, order_id).await?;
        orders.update_in_op(&mut op, &mut order).await?;
        op.commit().await?;

        Ok(())
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(entity = "Order")]
pub struct Orders {
    pool: PgPool,
    #[es_repo(nested)]
    items: OrderItems,
}

impl Orders {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool: pool.clone(),
            items: OrderItems::new(pool),
        }
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "OrderItem",
    columns(order_id(ty = "OrderId", update(persist = false), parent))
)]
pub struct OrderItems {
    pool: PgPool,
}

impl OrderItems {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[tokio::test]
async fn execute() -> anyhow::Result<()> {
    let pool = init_pool().await?;

    TestJob { pool }.execute().await?;

    Ok(())
}
