use std::cell::RefCell;
use std::io::Read;
use std::mem;

use xml::attribute::OwnedAttribute;
use xml::name::OwnedName;
use xml::namespace::Namespace;
use xml::ParserConfig;
use xml::reader as xml_reader;
use xml::reader::XmlEvent;

/// ElementSource is just a wrapper around XML reader, so we can wrap the Result + XML reader error too
pub type Result<T> = std::result::Result<T, xml_reader::Error>;

/// Produces elements from the provided source
pub struct ElementSource<R: Read> {
    // Needs to be a RefCell since we can't borrow mutably multiple times (e.g. when calls to Element::children() are nested)
    state: RefCell<SourceState<R>>,
}

impl<R: Read> ElementSource<R> {
    /// Parses the XML stream and emits elements
    ///
    /// # Arguments
    ///
    /// * `xml_data` - the data you wish to parse
    pub fn new(xml_data: R) -> ElementSource<R> {
        // Create the XML parser
        let config = ParserConfig::new()
            .trim_whitespace(true)
            .cdata_to_characters(true)
            .ignore_comments(true);
        let reader = xml_reader::EventReader::new_with_config(xml_data, config);

        // Initialise to a depth of 0 since traversing elements alter our depth (e.g. start element increases depth by 1)
        let state = RefCell::new(SourceState::new(reader));
        ElementSource { state }
    }

    /// Returns the first element in the source
    pub fn root(&self) -> Result<Option<Element<R>>> {
        self.next_element_at_depth(1)
    }

    // Returns the next element at the nominated depth
    fn next_element_at_depth(&self, iter_depth: u32) -> Result<Option<Element<R>>> {
        // Read nodes until we arrive at the correct depth
        let mut state = self.state.borrow_mut();
        while let Some(node) = state.next()? {
            match node {
                // The start of an element may be interesting to the iterator
                XmlEvent::StartElement { name, attributes, namespace } => {
                    // Starting an element increases our depth
                    state.current_depth += 1;

                    // If we are at the correct depth we found a node of interest
                    if state.current_depth == iter_depth {
                        let element = Element { name, attributes, namespace, source: &self, depth: state.current_depth };
                        return Ok(Some(element));
                    }
                }

                // The end of an element moves back up the hierarchy
                XmlEvent::EndElement { .. } => state.current_depth -= 1,

                // Not interested in other events when looking for elements
                _ => {}
            }

            // If we have hit the end of children at this level we terminate
            if state.current_depth < iter_depth - 1 {
                return Ok(None);
            }
        };

        // Hit the end of the document
        Ok(None)
    }

    // Extracts a text element
    fn text_node(&self) -> Result<Option<String>> {
        let mut state = self.state.borrow_mut();

        // If the next event is characters, we have found our text
        if let Ok(Some(XmlEvent::Characters(_text))) = state.peek() {
            // Grab the next event - we know its a Characters event from the above
            match state.next() {
                Ok(Some(XmlEvent::Characters(text))) => return Ok(Some(text)),
                _ => unreachable!("state.next() did not return expected XmlEvent::Characters")
            }
        }

        Ok(None)
    }
}

// Wraps the XML source and current depth of iteration
struct SourceState<R: Read> {
    reader: xml_reader::EventReader<R>,
    next: Result<Option<XmlEvent>>,
    current_depth: u32,
}

impl<R: Read> SourceState<R> {
    fn new(reader: xml_reader::EventReader<R>) -> SourceState<R> {
        let mut state = SourceState { reader, next: Ok(None), current_depth: 0 };
        state.next = state.fetch_next();
        state
    }

    // Returns the next event
    fn fetch_next(&mut self) -> Result<Option<XmlEvent>> {
        let reader = &mut self.reader;
        loop {
            let event = reader.next()?;
            match event {
                // Only interested in start + end + characters events
                XmlEvent::StartElement { .. } | XmlEvent::EndElement { .. } | XmlEvent::Characters(..) => { return Ok(Some(event)); }

                // If we hit the end of the document we have finished iteration
                XmlEvent::EndDocument => { return Ok(None); }

                // Ignore everything else
                _ => {}
            }
        }
    }

    // Returns the next interesting event (skips XmlEvent::StartDocument etc) or None if no more events are found
    fn next(&mut self) -> Result<Option<XmlEvent>> {
        let next = mem::replace(&mut self.next, Ok(None));
        self.next = self.fetch_next();
        next
    }

    // Peeks the next event (does not advance)
    // Callers should call next() to consume the event to move on
    fn peek(&mut self) -> &Result<Option<XmlEvent>> {
        &self.next
    }
}

/// An element exists at a given depth in the XML document hierarchy
pub struct Element<'a, R: Read> {
    /// Qualified name of the element.
    pub name: OwnedName,

    /// A list of attributes associated with the element.
    pub attributes: Vec<OwnedAttribute>,

    /// Contents of the namespace mapping at this point of the document.
    pub namespace: Namespace,

    // The underlying source of elements, allowing the more natural implementation of children() on an element rather than the source itself
    source: &'a ElementSource<R>,

    // Depth of this element
    depth: u32,
}

impl<'a, R: Read> Element<'a, R> {
    /// Returns an iterator over children of this element (i.e. descends a level in the hierarchy)
    pub fn children(&self) -> ElementIter<'a, R> {
        ElementIter { source: &self.source, depth: self.depth + 1 }
    }

    /// Returns the child of this element as a String
    pub fn child_as_text(&self) -> Result<Option<String>> {
        self.source.text_node()
    }
}

/// Iterator over elements at a specific depth in the hierarchy
pub struct ElementIter<'a, R: Read> {
    source: &'a ElementSource<R>,
    depth: u32,
}

impl<'a, R: Read> Iterator for ElementIter<'a, R> {
    type Item = Element<'a, R>;

    fn next(&mut self) -> Option<Self::Item> {
        self.source.next_element_at_depth(self.depth).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use crate::util::test;

    use super::*;

    type Result = std::result::Result<(), xml_reader::Error>;

    fn handle_book<R: Read>(book: Element<R>) -> Result {
        // Iterate over the children of the book
        let mut count = 0;
        for child in book.children() {
            match child.name.local_name.as_str() {
                "author" => {
                    count += 1;
                    assert_eq!(child.child_as_text()?.unwrap(), "Gambardella, Matthew");
                }
                "title" => {
                    count += 1;
                    assert_eq!(child.child_as_text()?.unwrap(), "XML Developer's Guide");
                }
                "nest1" => {
                    handle_nest1(child)?;
                }
                "empty1" | "empty2" => {
                    assert!(child.child_as_text()?.is_none());
                }
                _ => panic!("Unexpected child node: {}", child.name)
            }
        }

        // Should have found two elements
        assert_eq!(count, 2);

        Ok(())
    }

    fn handle_catalog<R: Read>(catalog: Element<R>) -> Result {
        // Iterate over the children of the catalog
        let mut count = 0;
        for child in catalog.children() {
            // First child should be book
            assert_eq!(child.name.local_name, "book");

            // Should have an id attribute
            assert!(child.attributes.iter().find(|attr| &attr.name.local_name == "id" && &attr.value == "bk101").is_some());

            // Should only have a single child at this level
            count += 1;

            // Handle the book
            handle_book(child)?;
        }
        assert_eq!(count, 1);

        Ok(())
    }

    fn handle_nest1<R: Read>(nest1: Element<R>) -> Result {
        // Should have a single child called "nest2"
        let mut count = 0;
        for child in nest1.children() {
            // First child should be nest2
            assert_eq!(child.name.local_name, "nest2");

            // It should have the expected text
            assert_eq!(child.child_as_text()?.unwrap(), "Nested");

            // Should only have a single child at this level
            count += 1;
        }
        assert_eq!(count, 1);

        Ok(())
    }

    #[test]
    fn test_iterate_stream() -> Result {
        let test_data = test::fixture_as_string("xml_sample.xml");

        // Root element should be "catalog"
        let source = ElementSource::new(test_data.as_bytes());
        let catalog = source.root()?.unwrap();
        assert_eq!(catalog.name.local_name, "catalog");
        handle_catalog(catalog)?;

        Ok(())
    }
}
