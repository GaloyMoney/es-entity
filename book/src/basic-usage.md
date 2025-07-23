# Basic Usage

This chapter covers the basic usage patterns of ES Entity.

## Creating Entities

```rust
// Create a new book
let book = Book::new(
    BookId::new(),
    "The Rust Programming Language",
    "Steve Klabnik and Carol Nichols",
);

// Persist to database
let repo = BookRepo::new(pool);
repo.create(book).await?;
```

## Loading Entities

```rust
// Find by ID
let book = repo.find_by_id(book_id).await?;

// List all books
let books = repo.list_all().await?;
```

## Updating Entities

```rust
// Load the book
let mut book = repo.find_by_id(book_id).await?;

// Make changes
book.publish();

// Save changes
repo.update(book).await?;
```

## Working with Events

All state changes are captured as events:

```rust
let events = book.events();
for event in events {
    match event {
        BookEvent::Created { title, author } => {
            println!("Book created: {} by {}", title, author);
        }
        BookEvent::Published => {
            println!("Book published!");
        }
    }
}
```